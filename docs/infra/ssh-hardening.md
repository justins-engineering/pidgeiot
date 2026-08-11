# SSH hardening: fail2ban replacing the connection-count throttle

The production VPS's `INPUT` chain (documented in
[`coap-terminator.md`](./coap-terminator.md#firewall) as the shared host
baseline) throttles SSH with the kernel's `xt_recent` module: six new
connections to port 22 from one source inside sixty seconds gets dropped.
That counts TCP *connections*, not authentication *failures* — a burst of
several short-lived, legitimate SSH sessions (a few automation invocations
back to back is enough) trips it exactly like a credential-guessing script
would, and the only fix once it triggers is waiting out the window. This
doc replaces that throttle with fail2ban, which parses sshd's own
auth-failure log lines and only acts on requests that actually failed to
authenticate.

The jail config lives at
[`infra/fail2ban/jail.local`](../../infra/fail2ban/jail.local) — no
secrets in it, so it's checked into the repo like the CoAP terminator's
compose fragment, deployed by hand to `/etc/fail2ban/jail.local`. Read
that file; the two choices below are explained in its comments too, this
is the fuller version.

## Two choices that matter more than they look

**Log source.** A stock Debian 13 host has no rsyslog installed, so
`/var/log/auth.log` doesn't exist — sshd logs to the systemd journal only.
fail2ban's `backend = auto` tries to tail a logfile and, finding none,
just... doesn't error. The jail reports healthy, matches nothing, bans
nothing, forever. `jail.local` sets `backend = systemd` explicitly to read
the journal directly. This is safe even if the host turns out to have
rsyslog after all — it just means both a journal and a flat file exist,
and systemd is still a valid source either way. What isn't safe is finding
out which is true only after the fact; that's what the verification step
below is for.

**Ban action.** Debian 13's fail2ban packaging can default to a native
nftables ban action. This host's firewall isn't managed that way — every
rule is authored and persisted through the `iptables`/`ip6tables` command
line plus `iptables-persistent` (`rules.v4`/`rules.v6`,
`netfilter-persistent save`). A ban inserted into a separate nftables
table would be invisible to `iptables -L -v -n` and untouched by
`netfilter-persistent save` — technically active, but disconnected from
every tool and habit this host's rules are normally audited and persisted
with. `jail.local` sets `banaction = iptables-multiport`, which shells out
to the same `iptables`/`ip6tables` binaries already in use, landing bans
in the same `INPUT` chain regardless of whether that binary happens to
be the iptables-legacy implementation or the iptables-nft compatibility
shim — both take the identical CLI syntax this action uses, so the choice
doesn't hinge on which one the host has. Confirm which is active with
`iptables --version` anyway, as a sanity check, not because it changes
anything.

## The operator has no static address

Most fail2ban write-ups tell you to put your own address in `ignoreip` and
move on. That option isn't available here: the admin network's IPv4 address
is dynamic, and fail2ban gives the operator no special treatment — five
failed authentications from the admin workstation get banned exactly like
five from a scanner. So the design assumes a self-ban will eventually
happen and makes it cheap, rather than assuming it can't.

Three things carry that weight, in place of an exemption.

The **first ban is ten minutes**, not an hour. Escalation still doubles per
repeat, so a real guessing attempt is quickly into hours, but a single
operator mistake costs a coffee break instead of an afternoon.

**Offer exactly one key.** With key-based auth the likely self-ban isn't a
mistyped password, it's an SSH agent holding several keys: the client
offers each in turn and each rejection is a separate authentication failure
in the log, so one connection attempt can burn most of `maxretry` on its
own. Pin the host to a single identity in `~/.ssh/config`:

```
Host <vps>
  IdentityFile ~/.ssh/<the_right_key>
  IdentitiesOnly yes
```

`IdentitiesOnly yes` is the load-bearing half — without it, ssh still
offers agent keys ahead of the configured one.

**Know the break-glass path before you need it.** A ban blocks port 22 and
nothing else, so recovery is the provider's out-of-band console (OVH KVM),
not another SSH attempt. Confirm that console access actually works *before*
applying any of this. From the console you can `fail2ban-client unban
--all`, or `systemctl stop fail2ban` to drop every ban at once.

If a dynamic-DNS hostname for the admin network ever exists, `ignoreip`
accepts hostnames and re-resolves them, which removes this whole problem.
Don't reach for the ISP's netblock as a substitute — it would exempt every
other customer on that ISP, and residential ranges are a meaningful share
of the traffic this jail exists to stop.

## Cutover procedure

Ordered so nothing can lock anyone out: fail2ban goes in and gets proven
working *first*, on top of the existing throttle, which keeps doing its
job the whole time. The throttle only comes out at the very end, once
fail2ban is confirmed to both detect and act.

### 1. Install, then check the two assumptions above against reality

```sh
apt update
apt install -y fail2ban python3-systemd
```

`python3-systemd` is only an `apt` *Recommends* of the `fail2ban` package,
not a hard dependency — install it explicitly rather than trusting that
Recommends weren't disabled somewhere on this host. It's what
`backend = systemd` actually needs to read the journal.

```sh
dpkg -s python3-systemd | grep Status        # confirms the journal bindings landed
systemctl status ssh --no-pager | head -3    # confirms the real unit name
test -f /var/log/auth.log \
  && echo "auth.log present (rsyslog installed)" \
  || echo "no auth.log — journal is the only source"
iptables --version                           # legacy vs nf_tables, informational
```

The unit name turns out not to be a real risk: the stock sshd filter's
journalmatch is `_SYSTEMD_UNIT=ssh.service + _COMM=sshd`, so it matches on
the process name even where the unit is called something else. Worth
knowing rather than worrying about — no `journalmatch` override is needed
on a stock Debian host, and adding one would *narrow* that match rather
than widen it.

A note on what a ban looks like, because it differs from the throttle being
replaced. `iptables-multiport`'s default blocktype is
`REJECT --reject-with icmp-port-unreachable`, not `DROP`. A banned client
gets an immediate "connection refused" instead of hanging until timeout —
which is a real diagnostic improvement: the silent timeouts the old
xt_recent `DROP` produced were exactly what made a self-inflicted lockout
look like an outage and invite the retry loop that prolonged it. If you are
banned by fail2ban, you will know within a second.

### 2. Deploy the jail

```sh
cp infra/fail2ban/jail.local /etc/fail2ban/jail.local
systemctl enable --now fail2ban
```

`jail.local` overrides the package's `jail.conf` by fail2ban's own
`.local`-over-`.conf` convention — nothing else needs editing.

### 3. Non-destructive verification — prove the filter matches real log lines

```sh
fail2ban-regex systemd-journal /etc/fail2ban/filter.d/sshd.conf \
  --journalmatch "_SYSTEMD_UNIT=ssh.service"
```

(swap the unit name if step 1 found a different one). Expected output is
a results table ending with something like:

```
Results
=======

Failregex: 4 total
Ignoreregex: 0 total
```

A nonzero `Failregex` count means it actually matched real lines from
this host's own journal — a public VPS has near-certainly already logged
some scanner noise, so zero is not "clean," it's the exact silent failure
this whole exercise exists to catch. Zero with no other error means the
`journalmatch` unit name is wrong; go back to step 1. A hard error instead
of a results table at all (e.g. "Unable to open journal") means the
systemd bindings or journal access is broken, independent of the filter —
fix that before trusting anything downstream of it.

Then confirm the jail process itself is healthy:

```sh
fail2ban-client status sshd
```

Expected: a `Status for the jail: sshd` block with `Currently failed` /
`Total failed` counters (matching the `fail2ban-regex` count once fail2ban
has had time to tail the same lines) and empty ban lists. `ERROR   Sorry
but the jail 'sshd' does not exist` means `jail.local` didn't parse —
`fail2ban-client -d` dumps the parsed config to see why.

### 4. Prove the ban action actually reaches this host's firewall

Do this with an address from RFC 5737's documentation range
(`192.0.2.0/24`, "TEST-NET-1") — it is never routable and never anyone's
real source, so this needs no failed login from the operator's own
address and cannot lock anyone out:

```sh
fail2ban-client set sshd banip 192.0.2.1
iptables -L -v -n | grep 192.0.2.1        # expect a match in the f2b-sshd chain
fail2ban-client set sshd unbanip 192.0.2.1
iptables -L -v -n | grep 192.0.2.1        # expect nothing now
```

Step 3 already proved detection; this proves the action half — that a ban
actually lands in, and is removed from, the exact chain
`iptables-persistent` will later save, which is the entire reason for
choosing `iptables-multiport` over the nftables default. If the first
`grep` comes back empty, the ban isn't reaching the firewall at all — stop
here, don't proceed to step 5, and see Rollback below.

While here, also confirm ordering:

```sh
iptables -L INPUT -v -n --line-numbers
```

The `f2b-sshd` jump rule needs to sit above the plain
`-p tcp --dport 22 -j ACCEPT` rule (rule 7 in the current baseline) — a
ban only matters if the chain checks it before that accept.

### 5. Remove the throttle — only once 3 and 4 both pass

`-D` (delete) takes the identical rule specification as `-A`/`-I`, not a
line number — line numbers shift the moment any earlier rule changes, but
matching by exact spec doesn't. `-D` requires that spec to match *exactly*
(argument order and spelling, including `--name` casing), so print the
live rules and confirm before deleting rather than assuming the text
below is verbatim what's installed:

```sh
iptables -S INPUT | grep -i recent
```

Delete using whatever that printout actually shows:

```sh
iptables -D INPUT -p tcp --dport 22 -m conntrack --ctstate NEW -m recent --set --name SSH
iptables -D INPUT -p tcp --dport 22 -m conntrack --ctstate NEW -m recent \
  --update --seconds 60 --hitcount 6 --name SSH -j DROP
```

Same for v6:

```sh
ip6tables -S INPUT | grep -i recent
ip6tables -D INPUT -p tcp --dport 22 -m conntrack --ctstate NEW -m recent --set --name SSH6
ip6tables -D INPUT -p tcp --dport 22 -m conntrack --ctstate NEW -m recent \
  --update --seconds 60 --hitcount 6 --name SSH6 -j DROP
```

Leave the plain `-p tcp --dport 22 -j ACCEPT` rule and the IPv6
`udp dpt:546` rule alone — only the two `recent`-module rules per family
are being replaced.

Verify:

```sh
iptables -L INPUT -v -n --line-numbers
ip6tables -L INPUT -v -n --line-numbers
```

No `recent: SSH` match should remain, and the `f2b-sshd` jump plus the
final ACCEPT should both still be present, in that order.

### 6. Persist

Stop fail2ban first. This is not optional, and it is the one step here
that is easy to get wrong in a way that only shows up weeks later:

```sh
systemctl stop fail2ban
netfilter-persistent save
systemctl start fail2ban
```

`netfilter-persistent save` snapshots the live ruleset, and a running
fail2ban has injected its own `f2b-sshd` chain, the `INPUT` jump into it,
and one rule per currently-banned address. Saving that state writes all of
it into `rules.v4`/`rules.v6` as if it were static configuration, which
breaks two ways at once. On the next boot the restore recreates the
`f2b-sshd` chain and its jump, then fail2ban starts and adds its own jump
on top — so the jumps accumulate one per reboot. And any address that
happened to be banned at save time is now a permanent static rule that
fail2ban has no record of, so `fail2ban-client unban` will not remove it
and neither will the ban expiring.

Stopping the service first runs the jail's `actionstop`, which tears down
the chain and the jump, leaving only the static ruleset to be saved.
Starting it again rebuilds them at runtime, which is where they belong.

One command covers both families — `netfilter-persistent` iterates every
registered plugin (iptables and ip6tables) on a single save, not two
separate invocations. Confirm the saved files contain neither the old
throttle nor any fail2ban state:

```sh
grep -iE 'recent|f2b' /etc/iptables/rules.v4 /etc/iptables/rules.v6
```

should return nothing from either file. If `f2b` appears, the save
happened while the service was running — stop it, save again, restart.

## Rollback

If step 3 or step 4 fails — the filter matches nothing real, or a test
ban never shows up in `iptables -L` — do not proceed to step 5. The
throttle is still fully in place at that point, so the only cleanup is
backing fail2ban back out:

```sh
systemctl disable --now fail2ban
iptables -L INPUT -v -n | grep f2b   # expect nothing — actionstop tears down
                                      # the jump rule and the f2b-sshd chain
```

If the problem only surfaces *after* step 5 (throttle already removed),
re-add the original pair ahead of the port-22 accept rule. Positions
shift once the throttle rules are gone, so locate the anchor rather than
assuming a fixed line number:

```sh
iptables -L INPUT --line-numbers -n | grep 'tcp dpt:22'   # find the plain ACCEPT rule, call its number N
iptables -I INPUT N -p tcp --dport 22 -m conntrack --ctstate NEW -m recent --set --name SSH
iptables -I INPUT N -p tcp --dport 22 -m conntrack --ctstate NEW -m recent \
  --update --seconds 60 --hitcount 6 --name SSH -j DROP
```

(insert the `--set` rule at `N` first so it lands immediately ahead of
the accept; inserting the `--update ... DROP` rule at that same `N`
afterward pushes the `--set` rule up by one, restoring the original
set-then-check order — confirm with `-L --line-numbers` between the two
inserts rather than assuming the second insert landed where expected).
Same pattern for `ip6tables` with `SSH6`, anchored on its own
`grep 'tcp dpt:22'`. Then stop fail2ban and `netfilter-persistent save`
again.

## fail2ban is noise reduction, not the control

Worth keeping in proportion. A `fail2ban-regex` run over this host's journal
matched roughly 253,000 genuine authentication failures against 96
successful logins — continuous, automated credential guessing, which is
simply the ambient condition for any VPS with port 22 reachable.

Against that, fail2ban's value is that it stops those attempts from
consuming resources and burying real events in log noise. It is not what
keeps the attacker out. What keeps them out is having nothing to guess:

```sh
sshd -T | grep -iE 'passwordauthentication|kbdinteractiveauthentication|permitrootlogin'
```

On this host that check returned `passwordauthentication yes` (with
`permitrootlogin without-password`, so root is already key-only). Those
attempts are therefore not bouncing off a wall — they are guesses against a
live door, on an image that ships a predictably-named `debian` account with
sudo. Turning password and keyboard-interactive auth off removes that
entire class outright; no amount of rate limiting is equivalent.

Two things make this change bite people, so do both.

**Confirm key auth works before removing the fallback.** There is no
password to fall back on afterward, and the only alternative is the
provider's console:

```sh
journalctl -u ssh --since "1 hour ago" | grep Accepted | tail -5
```

`Accepted publickey` is what you need to see. `Accepted password` means a
key isn't installed yet — fix that first.

**Put the setting where it actually wins.** sshd takes the
first-obtained value for each keyword, and `/etc/ssh/sshd_config` includes
`sshd_config.d/*.conf` at the top, read in alphabetical order. Cloud images
routinely ship a `50-cloud-init.conf` that sets `PasswordAuthentication
yes`, which is why editing `sshd_config` directly appears to do nothing —
the drop-in already won. Check what exists, then sort ahead of it:

```sh
grep -rn -i passwordauth /etc/ssh/sshd_config /etc/ssh/sshd_config.d/
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\n' \
  > /etc/ssh/sshd_config.d/10-no-password-auth.conf
sshd -t && systemctl reload ssh
sshd -T | grep -i passwordauthentication
```

`sshd -t` validates the config before it is applied, and `reload` leaves
established sessions alone. Keep the current session open and prove a new
one works from a second terminal before closing it.

Do this as its own change, after the cutover below is finished and
verified — locking down authentication and swapping the ban mechanism at
the same time makes it ambiguous which one caused any resulting lockout.

## Automation hygiene, independent of which throttle is in front

Whatever tripped the old connection-count throttle — several short-lived
`ssh` invocations landing inside the same sixty-second window — is worth
avoiding regardless of what's guarding port 22. Prefer one multiplexed
session (`ControlMaster`/`ControlPersist` in `ssh_config`) or a single
batched remote script over many separate `ssh` invocations for any future
automation against this host. fail2ban counts auth failures rather than
connections, so it won't reproduce today's specific lockout, but opening
a fresh TCP connection per command is still needless load and the kind of
pattern that looks like scanning from the outside.
