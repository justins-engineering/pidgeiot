#!/usr/bin/env bash
#
# proxmox-greptimedb-lxc.sh
#
# Provisions a Proxmox LXC container running GreptimeDB in STANDALONE mode,
# the default telemetry store for PidgeIoT (see docs/api.md + task #26).
#
# Run this ON THE PROXMOX HOST (needs `pct`/`pveam`), as root.
#
# OS CHOICE — Debian 13, not Alpine:
#   GreptimeDB publishes only glibc Linux binaries (greptime-linux-amd64 and a
#   CentOS/older-glibc variant) — there is NO musl build. On Alpine (musl) the
#   binary would need `gcompat` glibc-shimming, which is an unreliable bet for a
#   database. Debian 13 (Trixie) runs the stock glibc binary natively. If a musl
#   build ever ships, an Alpine variant of this script becomes trivial.
#
# SECURITY POSTURE (important — read before exposing):
#   PidgeIoT's design reaches this instance from a Cloudflare Worker at the edge,
#   via a Cloudflare Tunnel + Access service token (mirrors auth.pidgeiot.com;
#   the Worker cannot reach a LAN-internal host directly). GreptimeDB itself has
#   NO auth in standalone mode, so it must NOT sit exposed on an untrusted
#   network. Two options, controlled by BIND_ADDR below:
#     - BIND_ADDR=127.0.0.1  (MOST SECURE): GreptimeDB listens on loopback only;
#       run `cloudflared` INSIDE this container pointing at 127.0.0.1:4000 so the
#       tunnel is the ONLY ingress. Recommended for prod.
#     - BIND_ADDR=0.0.0.0    (reachable on the CT's LAN IP): convenient for
#       local testing / a host-side tunnel, but only acceptable on a trusted LAN
#       + Proxmox firewall. cloudflared can run on the host or this CT.
#   This script installs GreptimeDB only. Installing cloudflared is a separate,
#   documented step (see the end of this file).
#
set -euo pipefail

# ============================ CONFIG (edit me) ============================
CTID="${CTID:-105}"                       # Proxmox container ID (must be free)
HOSTNAME="${HOSTNAME:-greptimedb}"
# Debian 13 template. List/download with:
#   pveam update && pveam available --section system | grep debian-13
#   pveam download local debian-13-standard_13.<ver>_amd64.tar.zst
# then set TEMPLATE to the local: path it landed at (pveam list local).
TEMPLATE="${TEMPLATE:-local:vztmpl/debian-13-standard_13.1-2_amd64.tar.zst}"
ROOTFS_STORAGE="${ROOTFS_STORAGE:-local-lvm}"   # storage for the CT rootfs
DATA_STORAGE="${DATA_STORAGE:-$ROOTFS_STORAGE}" # storage for the telemetry data mount
ROOT_DISK_GB="${ROOT_DISK_GB:-8}"               # OS disk
DATA_DISK_GB="${DATA_DISK_GB:-32}"              # telemetry data disk (grow as needed)
CORES="${CORES:-2}"
RAM_MB="${RAM_MB:-2048}"
SWAP_MB="${SWAP_MB:-0}"
BRIDGE="${BRIDGE:-vmbr0}"
# Networking: DHCP by default. For a static IP set e.g.
#   IPCONFIG="ip=192.168.3.40/24,gw=192.168.3.1"
IPCONFIG="${IPCONFIG:-ip=dhcp}"
UNPRIVILEGED="${UNPRIVILEGED:-1}"               # 1 = unprivileged (recommended)
ONBOOT="${ONBOOT:-1}"

GREPTIME_VERSION="${GREPTIME_VERSION:-v1.1.3}"  # pin explicitly; matches docker-compose pin
GREPTIME_ARCH="${GREPTIME_ARCH:-greptime-linux-amd64}"  # or greptime-linux-amd64-centos on very old glibc
BIND_ADDR="${BIND_ADDR:-127.0.0.1}"               # see SECURITY POSTURE above; use 127.0.0.1 for tunnel-only
DATA_HOME="/var/lib/greptimedb"                 # telemetry data lives here (data disk mount)
# =========================================================================

log() { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

command -v pct >/dev/null || die "pct not found — run this on the Proxmox host as root."
pct status "$CTID" >/dev/null 2>&1 && die "CTID $CTID already exists. Pick a free ID or destroy it first."

log "Creating unprivileged LXC $CTID ($HOSTNAME) from $TEMPLATE"
pct create "$CTID" "$TEMPLATE" \
  --hostname "$HOSTNAME" \
  --cores "$CORES" --memory "$RAM_MB" --swap "$SWAP_MB" \
  --rootfs "${ROOTFS_STORAGE}:${ROOT_DISK_GB},mountoptions=noatime" \
  --mp0 "${DATA_STORAGE}:${DATA_DISK_GB},mp=${DATA_HOME},mountoptions=noatime" \
  --net0 "name=eth0,bridge=${BRIDGE},${IPCONFIG}" \
  --unprivileged "$UNPRIVILEGED" \
  --features nesting=1 \
  --onboot "$ONBOOT" \
  --start 1

log "Waiting for container network..."
for i in $(seq 1 30); do
  pct exec "$CTID" -- sh -c 'getent hosts github.com >/dev/null 2>&1' && break
  sleep 2
  [ "$i" = 30 ] && die "Container has no network/DNS after 60s — check BRIDGE/IPCONFIG."
done

# Release asset URLs — host-side template vars, expanded HERE by this script and
# interpolated into the pct-exec install block below (they are NOT container-
# local shell vars, so `${...}` resolves on the Proxmox host, not inside the CT).
# BASE is the full URL to the tarball (it embeds TARBALL). GreptimeDB publishes
# the checksum as a SIBLING asset named `<arch>-<version>.sha256sum` (NOT
# `<tarball>.sha256sum`), and its body is a BARE hash with no filename — both
# handled below.
REL="https://github.com/GreptimeTeam/greptimedb/releases/download/${GREPTIME_VERSION}"
TARBALL="${GREPTIME_ARCH}-${GREPTIME_VERSION}.tar.gz"
SHA_FILE="${GREPTIME_ARCH}-${GREPTIME_VERSION}.sha256sum"
BASE="${REL}/${TARBALL}"
SHA_URL="${REL}/${SHA_FILE}"

log "Installing base packages + GreptimeDB $GREPTIME_VERSION inside the container"
pct exec "$CTID" -- bash -euo pipefail -c "
  export DEBIAN_FRONTEND=noninteractive
  # Locale: Proxmox Debian templates generate no locale, so perl/apt emit
  # 'Setting locale failed' warnings (and some tools misbehave). Use C.UTF-8 —
  # always available with no locale-gen needed — and persist it. Approach from
  # community-scripts (github.com/community-scripts/ProxmoxVED, misc/install.func).
  export LC_ALL=C.UTF-8 LANG=C.UTF-8 LANGUAGE=C.UTF-8
  grep -qxF 'LC_ALL=C.UTF-8' /etc/environment 2>/dev/null || \
    printf 'LC_ALL=C.UTF-8\nLANG=C.UTF-8\n' >> /etc/environment
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates tar >/dev/null

  # dedicated unprivileged service user
  id greptime >/dev/null 2>&1 || useradd --system --home ${DATA_HOME} --shell /usr/sbin/nologin greptime
  install -d -o greptime -g greptime ${DATA_HOME}

  cd /tmp
  echo 'Downloading GreptimeDB + checksum...'
  curl -fsSL -o '${TARBALL}' '${BASE}'
  curl -fsSL -o '${SHA_FILE}' '${SHA_URL}'
  echo 'Verifying sha256...'
  # GreptimeDB's .sha256sum is a BARE hash (no filename), so pair it with the
  # tarball name before feeding it to 'sha256sum -c' (which needs '<hash>  <file>').
  echo \"\$(cat '${SHA_FILE}')  ${TARBALL}\" | sha256sum -c -    # aborts (set -e) on mismatch
  tar -xzf '${TARBALL}'
  # tarball extracts to a versioned dir containing the 'greptime' binary
  BIN=\$(find . -maxdepth 2 -name greptime -type f | head -1)
  [ -n \"\$BIN\" ] || { echo 'greptime binary not found in tarball' >&2; exit 1; }
  install -m 0755 \"\$BIN\" /usr/local/bin/greptime
  /usr/local/bin/greptime --version
  rm -rf /tmp/greptime*
"

log "Installing systemd service (binds ${BIND_ADDR}, data in ${DATA_HOME})"
pct exec "$CTID" -- bash -euo pipefail -c "cat > /etc/systemd/system/greptimedb.service <<UNIT
[Unit]
Description=GreptimeDB (standalone) — PidgeIoT telemetry store
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=greptime
Group=greptime
Environment=LC_ALL=C.UTF-8 LANG=C.UTF-8
# data_home defaults to ./greptimedb_data relative to WorkingDirectory, so this
# pins the telemetry data onto the ${DATA_HOME} mount (the data disk) reliably,
# without depending on a config-file schema that shifts between versions.
WorkingDirectory=${DATA_HOME}
ExecStart=/usr/local/bin/greptime standalone start \\
  --http-addr ${BIND_ADDR}:4000 \\
  --grpc-bind-addr ${BIND_ADDR}:4001 \\
  --mysql-addr ${BIND_ADDR}:4002 \\
  --postgres-addr ${BIND_ADDR}:4003
Restart=on-failure
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable --now greptimedb.service
"

log "Waiting for GreptimeDB HTTP health..."
IP=$(pct exec "$CTID" -- sh -c "ip -4 -o addr show eth0 | awk '{print \$4}' | cut -d/ -f1" | tr -d '\r')
for i in $(seq 1 30); do
  if pct exec "$CTID" -- sh -c 'curl -fsS http://127.0.0.1:4000/health >/dev/null 2>&1'; then
    log "GreptimeDB is UP."
    break
  fi
  sleep 2
  [ "$i" = 30 ] && die "GreptimeDB did not become healthy — check: pct exec $CTID -- journalctl -u greptimedb -e"
done

cat <<DONE

============================================================
 GreptimeDB standalone is running in LXC $CTID ($HOSTNAME)
   Container IP : ${IP:-<dhcp — check 'pct exec $CTID -- ip a'>}
   HTTP (ingest+SQL) : ${BIND_ADDR}:4000   (/v1/influxdb/write , /v1/sql , /health)
   gRPC              : ${BIND_ADDR}:4001
   MySQL wire        : ${BIND_ADDR}:4002
   Postgres wire     : ${BIND_ADDR}:4003
   Data home         : ${DATA_HOME}/greptimedb_data  (on the data disk)
   Service           : systemctl status greptimedb  |  journalctl -u greptimedb

 NEXT — expose it to the dovecote Worker via a Cloudflare Tunnel (task #26):
   1) Install cloudflared in this CT (or on the host) and create a tunnel:
        cloudflared tunnel login
        cloudflared tunnel create pidgeiot-telemetry
        # route a hostname (e.g. telemetry.pidgeiot.com) to http://${IP:-<CT-IP>}:4000
        #   (or http://127.0.0.1:4000 if cloudflared runs in THIS CT and you set BIND_ADDR=127.0.0.1)
        cloudflared tunnel route dns pidgeiot-telemetry telemetry.pidgeiot.com
        # run as a service: cloudflared service install
   2) In Cloudflare Zero Trust, put an ACCESS policy on telemetry.pidgeiot.com
      that requires a SERVICE TOKEN (so only dovecote, presenting
      CF-Access-Client-Id/Secret, can reach it — not the open internet).
   3) In dovecote wrangler.toml set (as SECRETS, not vars):
        GREPTIMEDB_ENDPOINT = https://telemetry.pidgeiot.com
        GREPTIMEDB_AUTH_TOKEN, GREPTIMEDB_ACCESS_CLIENT_ID, GREPTIMEDB_ACCESS_CLIENT_SECRET
      then deploy. (Design + fallback: task #26 / dovecote helpers/greptime.rs.)

 If BIND_ADDR=0.0.0.0 above, GreptimeDB is reachable on the CT's LAN IP with NO
 auth — lock it down (Proxmox firewall / co-locate cloudflared + set 127.0.0.1)
 before trusting it beyond a trusted LAN.
============================================================
DONE
