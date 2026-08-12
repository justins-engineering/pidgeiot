#!/usr/bin/env bash
# In-container orchestration for the RFC 9146 CID rebind harness. Runs
# INSIDE the privileged loft-cid-harness image (launcher: cid-netns-harness.sh);
# it stands up a cli<->nat<->srv netns topology over veth pairs with an
# nftables SNAT the harness flips mid-session, runs the real loft binary
# (LOFT_DTLS_STACK=mbedtls) against a stub PSK endpoint, drives the mbedTLS
# CID client through the NAT, and asserts on the client's result, the
# server-side wire capture, and loft's own journal.
#
# The assertions are built to FAIL a real regression, not just pass a happy
# path: every DTLS record in every datagram is walked (RFC 9146 puts the
# CID between the sequence number and the length, so a fixed-offset parser
# miscounts), a non-empty-capture precondition guards the zero-count
# checks, the rebind is proven independently from the wire (post-rebind
# source port), and the client's self-reported handshake count is
# cross-checked against loft's journal.
#
# Cells (design docs/infra/coap-cid-design.md, Tier 2):
#   1  CID rebind survival (primary): one routed datagram, no re-handshake;
#      uplink carries type-25 CID records, downlink stays type-23 only.
#   3  No-CID regression: the same rebind forces a re-handshake and no CID
#      record ever appears -- the current fleet's shape, bit for bit.
set -uo pipefail

# --- topology addresses ------------------------------------------------------
CLI_IP=10.1.0.2
NAT_CLI_IP=10.1.0.1
NAT_SRV_IP=10.2.0.1
SRV_IP=10.2.0.2
SNAT_PORT_A=40000
SNAT_PORT_B=40001
PSK_SECRET="cid-harness-secret-not-a-real-credential"
# Where preserved evidence goes when a cell fails; the launcher bind-mounts
# the host side and prints the path.
ARTIFACTS=/artifacts

WORK=$(mktemp -d)
FAILURES=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILURES=$((FAILURES + 1)); }
expect_eq() {
  if [ "$2" = "$3" ]; then pass "$1 ($2)"; else fail "$1 (got '$2', want '$3')"; return 1; fi
}
expect_ge() {
  if [ "${2:-x}" -ge "$3" ] 2>/dev/null; then pass "$1 ($2>=$3)"; else fail "$1 (got '$2', want >=$3)"; return 1; fi
}

cleanup() {
  for p in "${TRACKED_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  for ns in cli nat srv; do ip netns del "$ns" 2>/dev/null; done
  rm -rf "$WORK"
}
trap cleanup EXIT
declare -a TRACKED_PIDS=()

# A DTLS record walker that understands the RFC 9146 layout: for a
# tls12_cid record (content type 25) the 8-byte CID sits between the
# 6-byte sequence number and the 2-byte length, so its header is 21 bytes
# and its length lives at offset 19; every other record is a 13-byte
# header with length at offset 11. Emits one "REC <type>" per record, and
# "TAIL" for any datagram left with bytes the walk could not account for
# (a coalescing or parse mismatch -- must never happen on a clean run).
write_walker() {
  cat >"$WORK/walk.awk" <<'AWK'
function hexbyte(h, off) { return strtonum("0x" substr(h, off*2 + 1, 2)) }
{
  hex = $0; gsub(/:/, "", hex); gsub(/[[:space:]]/, "", hex)
  n = length(hex) / 2
  pos = 0; tail = 0
  while (pos < n) {
    type = hexbyte(hex, pos)
    if (type == 25) { hlen = 21; loff = 19 } else { hlen = 13; loff = 11 }
    if (pos + hlen > n) { tail = 1; break }
    reclen = hexbyte(hex, pos + loff) * 256 + hexbyte(hex, pos + loff + 1)
    if (pos + hlen + reclen > n) { tail = 1; break }
    print "REC " type
    pos += hlen + reclen
  }
  if (tail) print "TAIL"
}
AWK
}

# Every DTLS record (as "REC <type>"/"TAIL" lines) on 5684 matching a
# display filter, walked full-datagram from the raw UDP payload -- so the
# assertions never depend on tshark's own DTLS dissector.
walk_dir() {
  local pcap="$1" filter="$2"
  tshark -r "$pcap" -Y "udp.port==5684 && ($filter)" -T fields -e udp.payload 2>/dev/null \
    | gawk -f "$WORK/walk.awk"
}
count_type() { grep -c "^REC $2\$" <<<"$1"; }
count_tail() { grep -c "^TAIL\$" <<<"$1"; }
count_rec() { grep -c "^REC " <<<"$1"; }

setup_topology() {
  ip netns add cli
  ip netns add nat
  ip netns add srv

  ip link add veth-cli type veth peer name veth-natc
  ip link add veth-srv type veth peer name veth-nats
  ip link set veth-cli netns cli
  ip link set veth-natc netns nat
  ip link set veth-nats netns nat
  ip link set veth-srv netns srv

  ip -n cli addr add "$CLI_IP/24" dev veth-cli
  ip -n cli link set veth-cli up
  ip -n cli link set lo up
  ip -n cli route add default via "$NAT_CLI_IP"

  ip -n nat addr add "$NAT_CLI_IP/24" dev veth-natc
  ip -n nat addr add "$NAT_SRV_IP/24" dev veth-nats
  ip -n nat link set veth-natc up
  ip -n nat link set veth-nats up
  ip -n nat link set lo up
  # Enable forwarding by writing /proc directly (no dependency on a sysctl
  # binary that the slim image may not carry) and ASSERT it took -- the
  # NAT is useless without it, and a silent failure would make the whole
  # gate rest on a fresh netns happening to inherit the value.
  ip netns exec nat sh -c 'echo 1 > /proc/sys/net/ipv4/ip_forward'
  local fwd
  fwd=$(ip netns exec nat cat /proc/sys/net/ipv4/ip_forward)
  expect_eq "topology: nat ip_forward enabled" "$fwd" "1"

  ip -n srv addr add "$SRV_IP/24" dev veth-srv
  ip -n srv link set veth-srv up
  ip -n srv link set lo up

  ip netns exec nat nft add table ip nat
  ip netns exec nat nft "add chain ip nat post { type nat hook postrouting priority srcnat ; }"
}

set_snat() {
  # Force the client's source to an exact NAT port so a "rebind" is a
  # deterministic port change. The whole swap is one atomic `nft -f`
  # transaction (flush-chain + add-rule): a non-atomic swap would leave a
  # window where a client datagram leaks through un-NAT'd, a harness
  # artifact rather than device behaviour. The udp match is also what
  # makes the :port snat target valid.
  local port="$1"
  ip netns exec nat nft -f - <<EOF
flush chain ip nat post
add rule ip nat post ip saddr $CLI_IP udp dport 5684 oifname veth-nats snat ip to $NAT_SRV_IP:$port
EOF
}

# --- one cell ----------------------------------------------------------------
run_cell() {
  local name="$1" mode="$2" expect_handshakes="$3"
  echo "=== cell: $name (mode=$mode) ==="
  local loftlog="$WORK/loft-$mode.log"
  local stublog="$WORK/stub-$mode.log"
  local pcap="$WORK/cap-$mode.pcap"
  local cliout="$WORK/cli-$mode.out"

  set_snat "$SNAT_PORT_A"
  # Clean conntrack slate per cell: the nat namespace is shared across
  # cells, so a prior cell's flow entries must not carry into this one.
  ip netns exec nat conntrack -F >/dev/null 2>&1

  ip netns exec srv /usr/local/bin/psk_stub 127.0.0.1:8788 >"$stublog" 2>&1 &
  local stub_pid=$!
  sleep 0.3
  COAP_SERVICE_SECRET="$PSK_SECRET" \
    LOFT_DTLS_STACK=mbedtls \
    LOFT_UDP_LISTEN="0.0.0.0:5684" \
    LOFT_TCP_LISTEN="127.0.0.1:5684" \
    LOFT_DOVECOTE_URL="http://127.0.0.1:8788" \
    LOFT_LOG=info \
    ip netns exec srv /usr/local/bin/loft >"$loftlog" 2>&1 &
  local loft_pid=$!
  sleep 1.0

  ip netns exec srv tshark -i veth-srv -w "$pcap" -f "udp port 5684" >/dev/null 2>&1 &
  local cap_pid=$!
  sleep 0.5

  # Bounded so a wedged client can never hang the gate.
  timeout 120 ip netns exec cli /usr/local/bin/cid_client \
    --target "$SRV_IP:5684" --mode "$mode" --exchanges 8 --interval-ms 500 \
    >"$cliout" 2>&1 &
  local cli_pid=$!
  TRACKED_PIDS=("$stub_pid" "$loft_pid" "$cap_pid" "$cli_pid")

  # Event-driven rebind: wait until at least three stable exchanges have
  # landed on the first port, then flip -- instead of a fixed sleep whose
  # margin shrinks on a loaded box.
  local rebind_deadline=$((SECONDS + 40))
  until grep -q '^EXCHANGE 2 ok' "$cliout" 2>/dev/null; do
    if ! kill -0 "$cli_pid" 2>/dev/null; then break; fi
    if [ "$SECONDS" -ge "$rebind_deadline" ]; then
      echo "  (timed out waiting for pre-rebind exchanges)"
      break
    fi
    sleep 0.1
  done
  echo "--- rebind: SNAT $SNAT_PORT_A -> $SNAT_PORT_B, flush conntrack ---"
  set_snat "$SNAT_PORT_B"
  ip netns exec nat conntrack -F >/dev/null 2>&1

  wait "$cli_pid" 2>/dev/null
  sleep 0.5
  kill "$cap_pid" 2>/dev/null; wait "$cap_pid" 2>/dev/null
  kill "$loft_pid" 2>/dev/null; wait "$loft_pid" 2>/dev/null
  kill "$stub_pid" 2>/dev/null; wait "$stub_pid" 2>/dev/null
  TRACKED_PIDS=()

  echo "--- client output ---"; sed 's/^/  /' "$cliout"

  # --- parse ---
  local result handshakes ok total
  result=$(grep '^RESULT' "$cliout" | tail -1)
  handshakes=$(sed -n 's/.*handshakes=\([0-9]*\).*/\1/p' <<<"$result")
  ok=$(sed -n 's#.*exchanges_ok=\([0-9]*\)/\([0-9]*\).*#\1#p' <<<"$result")
  total=$(sed -n 's#.*exchanges_ok=\([0-9]*\)/\([0-9]*\).*#\2#p' <<<"$result")

  local up_all down_all up_pre up_post
  up_all=$(walk_dir "$pcap" "ip.src==$NAT_SRV_IP")
  down_all=$(walk_dir "$pcap" "ip.src==$SRV_IP")
  up_pre=$(walk_dir "$pcap" "ip.src==$NAT_SRV_IP && udp.srcport==$SNAT_PORT_A")
  up_post=$(walk_dir "$pcap" "ip.src==$NAT_SRV_IP && udp.srcport==$SNAT_PORT_B")

  # --- exchange success + handshake count: these read the client output
  #     and loft's journal, not the capture, so they run unconditionally. ---
  expect_eq "$name: all exchanges succeeded across the rebind" "$ok" "$total"
  expect_eq "$name: client handshake count" "$handshakes" "$expect_handshakes"
  expect_eq "$name: loft journal established-session count" \
    "$(grep -c 'DTLS session established' "$loftlog")" "$expect_handshakes"

  # --- journal-based CID oracles: independent of the capture (they read
  #     loft's own log), so they run unconditionally and catch a neutered
  #     rebind even if the capture were also broken. Exactly one, not
  #     merely present. ---
  if [ "$mode" = cid ]; then
    expect_eq "$name: exactly one CID-negotiated session in journal" \
      "$(grep -c 'CID negotiated' "$loftlog")" 1
    expect_eq "$name: exactly one address migration in journal" \
      "$(grep -c 'address migration' "$loftlog")" 1
  fi

  # --- wire preconditions: the capture must exist and parse cleanly.
  #     ALL wire assertions are gated on this so an empty or stubbed
  #     capture can never satisfy a "== 0" check vacuously -- it fails the
  #     precondition and the zero-checks are skipped, not silently passed.
  #     The post-rebind uplink must be non-empty too: a session that sent
  #     nothing on the new port never actually took the rebind. ---
  local up_rec down_rec post_rec wire_ok=1
  up_rec=$(count_rec "$up_all")
  down_rec=$(count_rec "$down_all")
  post_rec=$(count_rec "$up_post")
  expect_ge "$name: uplink capture is non-empty" "$up_rec" 1 || wire_ok=0
  expect_ge "$name: downlink capture is non-empty" "$down_rec" 1 || wire_ok=0
  expect_ge "$name: post-rebind uplink is non-empty (rebind actually taken)" "$post_rec" 1 || wire_ok=0

  if [ "$wire_ok" -ne 1 ]; then
    fail "$name: wire assertions SKIPPED -- capture empty/unparsable, cannot verify record-level properties"
  else
    expect_eq "$name: no unparsed uplink record tails" "$(count_tail "$up_all")" 0
    expect_eq "$name: no unparsed downlink record tails" "$(count_tail "$down_all")" 0

    # Device-safety: the server NEVER emits a CID-bearing downlink record,
    # at ANY record position (the device blackholes those).
    expect_eq "$name: zero type-25 downlink records (device-safe)" "$(count_type "$down_all" 25)" 0
    expect_ge "$name: downlink carried type-23 application records" "$(count_type "$down_all" 23)" 1

    if [ "$mode" = cid ]; then
      # CID engaged on BOTH the pre- and post-rebind source ports -- the
      # post-rebind evidence is what proves the session actually survived
      # the rebind, not merely that CID engaged before it.
      expect_ge "$name: pre-rebind uplink type-25 records" "$(count_type "$up_pre" 25)" 1
      expect_ge "$name: post-rebind uplink type-25 records (survived on the new port)" \
        "$(count_type "$up_post" 25)" 1
      # Zero ClientHellos (handshake records) after the rebind = no re-handshake.
      expect_eq "$name: zero post-rebind uplink handshake records (no re-handshake)" \
        "$(count_type "$up_post" 22)" 0
    else
      # Never any CID record, either direction.
      expect_eq "$name: zero type-25 uplink records (CID never negotiated)" \
        "$(count_type "$up_all" 25)" 0
      # The rebind oracle for the no-CID cell: recovery REQUIRED a fresh
      # handshake, visible as post-rebind handshake records on the new port.
      expect_ge "$name: post-rebind uplink handshake records (re-handshake forced)" \
        "$(count_type "$up_post" 22)" 1
    fi
  fi

  # --- stub health: a leaked stub from a prior cell would AddrInUse-panic
  #     this one, and loft would then be resolving against the wrong
  #     process. Catch it explicitly. ---
  if grep -qi 'panic' "$stublog"; then
    fail "$name: psk_stub panicked (leaked stub / AddrInUse?): $(head -1 "$stublog")"
  else
    pass "$name: psk_stub healthy"
  fi
  echo
}

# --- run ---------------------------------------------------------------------
write_walker
setup_topology
run_cell "cid-rebind-survival" cid 1
run_cell "no-cid-regression" nocid 2

echo "==================================="
if [ "$FAILURES" -eq 0 ]; then
  echo "ALL CELLS PASSED"
  exit 0
fi

# Preserve evidence for a failing run before the EXIT trap wipes $WORK.
if [ -d "$ARTIFACTS" ]; then
  cp -r "$WORK/." "$ARTIFACTS/" 2>/dev/null \
    && echo "$FAILURES ASSERTION(S) FAILED -- evidence copied to the mounted artifacts dir" \
    || echo "$FAILURES ASSERTION(S) FAILED (artifact copy failed)"
else
  echo "$FAILURES ASSERTION(S) FAILED (no artifacts mount; rerun via cid-netns-harness.sh to preserve evidence)"
fi
exit 1
