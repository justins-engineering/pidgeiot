#!/usr/bin/env python3
"""Delete fixture (or erased) accounts without leaving orphans behind.

A Kratos identity is not the account. Deleting the identity alone strands
everything keyed on its id: flocks, pigeons, access lists, alerts, orgs,
memberships, invitations, consent rows, saved dashboard state, error
reports and usage rows -- and, worse, every one of that account's pigeons
keeps a live Durable Object that no route can ever reach again, because
`DELETE /pigeons/:id` is the only thing that empties one and it needs a
session for an identity on that pigeon's own access list.

So the order is forced and this script exists to hold it: inventory first,
then delete through the product's own routes as the account, then sweep
what no route reaches, then prove by direct psql that nothing is left, and
only then the identity -- proving it afterwards would report the problem
one step too late to fix.

  scripts/purge-identities.py --env dev --identities ids.txt
  scripts/purge-identities.py --env both --candidates-from identities.json
  scripts/purge-identities.py --env both --candidates-from ids.json --apply

Dry run is what happens without --apply, which asks for the plan's ready
count typed back on a terminal. One Kratos serves staging and production,
so `both` is one identity list against two databases -- and the only --env
that can finish an account holding rows in each, since rows in a database
the run did not select are a hold.

Connection strings are read by name from the environment, else from
`secrets.env`, and are decomposed into PG* variables so they never reach
argv. Recovery links reach the remote shell in a file it writes itself, so
no token lands in either machine's argv. No connection string, cookie or
token is printed or logged.

Docs: docs/infra/account-deletion.md is the procedure this implements and
the DPA's Annex II G.3 refers to.
"""

import argparse
import datetime
import json
import os
import re
import shlex
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

# Refused under every flag. The two standing staging fixtures are here
# because the e2e suites re-enter them by recovery rather than recreating
# them; losing either costs a bench day.
KEEP_EMAILS = frozenset({
  "code@jes.contact",
  "justin@jes.contact",
  "code+staging-e2e-20260826@jes.contact",
  "code+staging-e2e-b-20260826@jes.contact",
})

# Skipped with its reason printed; purged only with --include-held.
HELD_EMAILS = {
  "staging-catch@pidgeiot.com": "the standing mail-catcher recipient staging alerts are sent to",
}

# build-notice-list.sh's own rule. An address failing all four is a real
# person's, and the notice list was built from exactly this split.
INTERNAL_PATTERNS = (
  re.compile(r"@(jes\.contact|pidgeiot\.com)$"),
  re.compile(r"@(example\.com|localhost)$"),
  re.compile(r"\+(staging-)?e2e"),
  re.compile(r"^(test|fixture|smoke)[-+.]"),
)

# The inventory this was written against. A mismatch means identities.json
# is stale, which is the one thing that could put a real account in a plan.
EXPECTED_CLASSES = {"external": 6, "keep": 4, "held": 1, "candidate": 42}

UUID_RE = re.compile(r"^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$", re.IGNORECASE)

# Stands in for an address the run does not have, so an email-keyed
# statement matches nothing instead of matching every empty string.
NO_EMAIL = "(no address on file)"

# capsules::TERMS_OF_SERVICE_PURPOSE.
TERMS_PURPOSE = "terms_of_service"

# wrangler.toml's [[env.dev.hyperdrive]] localConnectionString.
DEV_PG_FALLBACK = "postgres://kratos:secret@127.0.0.1:5432/dovecote?sslmode=disable"

# name -> (api host, connection-string variable, realm)
ENVIRONMENTS = {
  "dev": ("http://127.0.0.1:8787", "PIDGEIOT_DEV_PSQL_CONNECTION", "dev"),
  "staging": ("https://api-staging.pidgeiot.com", "STAGING_PSQL_CONNECTION", "prod"),
  "prod": ("https://api.pidgeiot.com", "DOVECOTE_PSQL_CONNECTION", "prod"),
}

# realm -> (kratos public base, kratos admin base, ssh target variable)
# Production Kratos binds both listeners to loopback, so its admin API is
# reachable only through one ssh hop.
REALMS = {
  "dev": ("http://127.0.0.1:4433", "http://127.0.0.1:4434", None),
  "prod": ("http://127.0.0.1:4433", "http://127.0.0.1:4434", "VPS_SSH"),
}


def die(msg):
  print(f"error: {msg}", file=sys.stderr)
  sys.exit(1)


# --- inputs ------------------------------------------------------------


def load_identities(path):
  """Read a Kratos admin identity dump: JSON lines, or concatenated pages."""
  with open(path, encoding="utf-8") as fh:
    text = fh.read()
  decoder = json.JSONDecoder()
  out = []
  pos = 0
  while True:
    while pos < len(text) and text[pos] in " \t\r\n,":
      pos += 1
    if pos >= len(text):
      break
    value, pos = decoder.raw_decode(text, pos)
    out.extend(value if isinstance(value, list) else [value])
  return [{
    "id": i["id"],
    "email": (i.get("traits", {}).get("email") or "").strip().lower(),
    "state": i.get("state", "unknown"),
  } for i in out]


def classify_email(email, keeps, held):
  """One of external, keep, held, candidate."""
  email = (email or "").strip().lower()
  if email in keeps:
    return "keep"
  if not any(p.search(email) for p in INTERNAL_PATTERNS):
    return "external"
  if email in held:
    return "held"
  return "candidate"


def read_secret(name, repo_root):
  """Environment first, then the repo's gitignored secrets.env. Never printed."""
  value = os.environ.get(name, "").strip()
  if value:
    return value
  path = os.path.join(repo_root, "secrets.env")
  if not os.path.exists(path):
    return ""
  with open(path, encoding="utf-8") as fh:
    for line in fh:
      line = line.strip()
      if not line or line.startswith("#") or "=" not in line:
        continue
      key, _, raw = line.partition("=")
      if key.strip() == name:
        return raw.strip().strip('"').strip("'")
  return ""


def pg_env(uri):
  """Decompose a connection string into PG* variables, keeping it out of argv."""
  parts = urllib.parse.urlsplit(uri)
  if parts.scheme not in ("postgres", "postgresql") or not parts.hostname:
    die("could not parse a postgres connection string")
  env = {
    "PGHOST": parts.hostname,
    "PGPORT": str(parts.port or 5432),
    "PGDATABASE": urllib.parse.unquote(parts.path.lstrip("/")) or "postgres",
    "PGUSER": urllib.parse.unquote(parts.username or ""),
    "PGPASSWORD": urllib.parse.unquote(parts.password or ""),
  }
  sslmode = urllib.parse.parse_qs(parts.query).get("sslmode")
  if sslmode:
    env["PGSSLMODE"] = sslmode[0]
  return env


# --- database ----------------------------------------------------------


class Database:
  """One psql connection target. Values go in -v bindings, read as :'name'."""

  def __init__(self, name, uri):
    self.name = name
    self._env = pg_env(uri)

  def rows(self, sql, **params):
    args = ["psql", "-X", "-q", "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1"]
    for key, value in params.items():
      args += ["-v", f"{key}={value}"]
    args += ["-f", "-"]
    proc = subprocess.run(args, input=sql, capture_output=True, text=True,
                          env=dict(os.environ, **self._env))
    if proc.returncode != 0:
      die(f"psql failed against {self.name}: {proc.stderr.strip()[:300]}")
    return [line.split("\t") for line in proc.stdout.splitlines() if line != ""]

  def count(self, sql, **params):
    rows = self.rows(sql, **params)
    return int(rows[0][0]) if rows else 0

  def script(self, sql, **params):
    """One statement batch in a single transaction."""
    return self.rows("BEGIN;\n" + sql + "\nCOMMIT;\n", **params)

  def opened(self):
    """Server and database, so two environments cannot silently share one."""
    return tuple(self.rows("SELECT current_database(),"
                           " coalesce(inet_server_addr()::text, 'local'), inet_server_port();")[0])


# --- http --------------------------------------------------------------


class NoRedirect(urllib.request.HTTPRedirectHandler):
  def redirect_request(self, req, fp, code, msg, headers, newurl):
    return None


# Cloudflare's Browser Integrity Check refuses Python's default agent with a 1010
# before the worker sees the request; curl on the VPS and browsers pass.
USER_AGENT = "pidgeiot-ops/1 (+https://pidgeiot.com)"


def http(method, url, body=None, headers=None, follow=True):
  """Returns (status, header list of (name, value), text body)."""
  request = urllib.request.Request(url, data=body, method=method,
                                   headers={"User-Agent": USER_AGENT, **(headers or {})})
  opener = urllib.request.build_opener(*([] if follow else [NoRedirect]))
  try:
    with opener.open(request, timeout=30) as resp:
      return resp.status, list(resp.headers.items()), resp.read().decode(errors="replace")
  except urllib.error.HTTPError as e:
    return e.code, list(e.headers.items()), e.read().decode(errors="replace")
  except OSError as e:
    die(f"{method} {url.split('?')[0]} failed: {e}")


class Kratos:
  """The admin and public APIs, locally or through one multiplexed ssh hop."""

  def __init__(self, realm, repo_root, run_dir):
    self.public, self.admin, ssh_var = REALMS[realm]
    self.ssh = read_secret(ssh_var, repo_root) if ssh_var else ""
    if ssh_var and not self.ssh:
      die(f"{ssh_var} is not set; the production Kratos admin API is loopback only")
    self._control = os.path.join(run_dir, "ssh-control")

  def call(self, method, url, body=None, follow=True):
    if not self.ssh:
      headers = {"Content-Type": "application/json"} if body else {}
      return http(method, url, body.encode() if body else None, headers, follow)
    return self._remote(method, url, body, follow)

  def remote_script(self, method, url, body, follow):
    """The shell the ssh hop runs. The url and body are files it writes from
    quoted heredocs: a one-time recovery token belongs in neither machine's
    argv, and curl cannot read a body from the pipe this script arrives on."""
    lines = ["set -eu", "b=$(mktemp)", "h=$(mktemp)", "k=$(mktemp)", "d=",
             'trap \'rm -f "$b" "$h" "$k" $d\' EXIT',
             'cat >"$k" <<\'PURGE_CONF\'', "url = " + json.dumps(url), "PURGE_CONF"]
    data = ""
    if body:
      lines += ["d=$(mktemp)", 'cat >"$d" <<\'PURGE_BODY\'', body, "PURGE_BODY"]
      data = '-H \'Content-Type: application/json\' --data-binary @"$d"'
    lines += ["code=$(curl -sS -K \"$k\" -X %s -o \"$b\" -D \"$h\" -w '%%{http_code}' %s %s)" % (
                shlex.quote(method), "-L" if follow else "", data),
              "printf '%s\\n' \"$code\"", "printf -- '--headers--\\n'", 'cat "$h"',
              "printf -- '--body--\\n'", 'cat "$b"']
    return "\n".join(lines) + "\n"

  def _remote(self, method, url, body, follow):
    args = ["ssh", "-o", "BatchMode=yes", "-o", "ControlMaster=auto",
            "-o", f"ControlPath={self._control}", "-o", "ControlPersist=120",
            self.ssh, "sh", "-s"]
    proc = subprocess.run(args, input=self.remote_script(method, url, body, follow),
                          capture_output=True, text=True)
    if proc.returncode != 0:
      die(f"ssh curl failed: {proc.stderr.strip()[:300]}")
    head, marker, rest = proc.stdout.partition("--headers--\n")
    if not marker or not head.strip():
      die(f"the remote shell produced no status line for {method} "
          f"{url.split('?')[0]}; nothing was read")
    raw_headers, _, text = rest.partition("--body--\n")
    headers = []
    for line in raw_headers.splitlines():
      if ":" in line:
        name, _, value = line.partition(":")
        headers.append((name.strip(), value.strip()))
    return int(head.strip().splitlines()[-1]), headers, text

  def mint_session(self, identity_id):
    """A one-time admin recovery link, spent once. Queues no mail."""
    body = json.dumps({"identity_id": identity_id, "expires_in": "10m"})
    status, _, text = self.call("POST", self.admin + "/admin/recovery/link", body)
    if status != 200:
      die(f"recovery link for {identity_id} -> HTTP {status}")
    link = json.loads(text)["recovery_link"]
    parts = urllib.parse.urlsplit(link)
    base = urllib.parse.urlsplit(self.public)
    # Spend it against the loopback listener: the token then never crosses
    # the tunnel and never lands in an edge log.
    spend = urllib.parse.urlunsplit((base.scheme, base.netloc, parts.path,
                                     parts.query, parts.fragment))
    status, headers, _ = self.call("GET", spend, follow=False)
    for name, value in headers:
      if name.lower() == "set-cookie" and value.startswith("ory_kratos_session="):
        return value.split(";", 1)[0].split("=", 1)[1]
    die(f"recovery for {identity_id} answered HTTP {status} with no session cookie")

  def revoke_sessions(self, identity_id):
    return self.call("DELETE", f"{self.admin}/admin/identities/{identity_id}/sessions")[0]

  def delete_identity(self, identity_id):
    return self.call("DELETE", f"{self.admin}/admin/identities/{identity_id}")[0]

  def identity_status(self, identity_id):
    return self.call("GET", f"{self.admin}/admin/identities/{identity_id}")[0]

  def all_identities(self):
    out = []
    url = f"{self.admin}/admin/identities?page_size=250"
    while url:
      status, headers, text = self.call("GET", url)
      if status != 200:
        die(f"listing identities -> HTTP {status}")
      out.extend(json.loads(text))
      url = ""
      for name, value in headers:
        if name.lower() == "link":
          match = re.search(r'<([^>]+)>;\s*rel="next"', value)
          if match and "page_token=00000000-0000-0000-0000-000000000000" not in match.group(1):
            url = urllib.parse.urljoin(self.admin, match.group(1))
    return out


# --- inventory ---------------------------------------------------------

# Every table in footprint.md's identity-bearing list, read straight from
# the database: a list route would be answered from Hyperdrive's ~60s
# query cache and cannot be trusted either side of a write.
INVENTORY_QUERIES = {
  "flocks": "SELECT f.id, f.name, coalesce(f.org_id::text, ''),"
            " (SELECT count(*) FROM pigeons p WHERE p.flock_id = f.id)::text"
            " FROM flocks f WHERE f.user_id = :'id'::uuid ORDER BY f.id",
  "pigeons": "SELECT p.id, p.flock_id::text FROM pigeons p"
             " JOIN flocks f ON f.id = p.flock_id WHERE f.user_id = :'id'::uuid ORDER BY p.id",
  "acl": "SELECT a.id, a.role, f.user_id = :'id'::uuid FROM pigeon_acl a"
         " JOIN pigeons p ON p.id = a.id JOIN flocks f ON f.id = p.flock_id"
         " WHERE a.entity_id = :'id'::uuid ORDER BY a.id",
  # Grants OTHER entities hold on the pigeons this run would delete. The
  # keep list protects accounts, not the devices they were granted.
  "acl_others": "SELECT a.id, a.entity_id::text, a.role FROM pigeon_acl a"
                " JOIN pigeons p ON p.id = a.id JOIN flocks f ON f.id = p.flock_id"
                " WHERE f.user_id = :'id'::uuid AND a.entity_id <> :'id'::uuid"
                " ORDER BY a.id, a.entity_id",
  "orgs": "SELECT m.org_id::text, m.role, o.name,"
          " coalesce(o.stripe_customer_id, ''), coalesce(o.stripe_subscription_id, ''),"
          " (SELECT count(*) FROM organization_members m2 WHERE m2.org_id = m.org_id)::text,"
          " (SELECT count(*) FROM organization_members m3 WHERE m3.org_id = m.org_id"
          "   AND m3.role = 'owner')::text,"
          " (SELECT count(*) FROM flocks f WHERE f.org_id = m.org_id)::text"
          " FROM organization_members m JOIN organizations o ON o.id = m.org_id"
          " WHERE m.user_id = :'id'::uuid ORDER BY m.org_id",
  "alerts": "SELECT id::text, name FROM alert_definitions WHERE user_id = :'id'::uuid ORDER BY id",
  "dashboard_state": "SELECT scope_key FROM dashboard_state WHERE user_id = :'id'::uuid"
                     " ORDER BY scope_key",
  "invites": "SELECT id::text, org_id::text, lower(email),"
             " accepted_at IS NOT NULL OR expires_at <= now()"
             " FROM organization_invites"
             " WHERE created_by = :'id'::uuid OR lower(email) = :'email' ORDER BY id",
  "consent": "SELECT seq::text, purpose, source, notice_version FROM consent_events"
             " WHERE identity_id = :'id'::uuid ORDER BY seq",
  "errors": "SELECT count(*)::text FROM error_events WHERE user_id = :'id'::uuid",
  "contact": "SELECT count(*)::text FROM contact_submissions WHERE user_id = :'id'::uuid",
  "usage": "SELECT count(*)::text FROM billing_usage_periods"
           " WHERE owner_kind = 'user' AND owner_id = :'id'::uuid",
  "member_refs": "SELECT org_id::text, user_id::text FROM organization_members"
                 " WHERE invited_by = :'id'::uuid OR"
                 " (lower(email) = :'email' AND user_id <> :'id'::uuid) ORDER BY org_id",
  "owner_email": "SELECT count(*)::text FROM flocks"
                 " WHERE lower(owner_email) = :'email' AND user_id <> :'id'::uuid",
  # An email channel is a reference to a person no id column carries.
  # `position` rather than LIKE: an address may hold a `_` or a `%`.
  "alert_recipients": "SELECT id::text, user_id::text FROM alert_definitions"
                      " WHERE user_id <> :'id'::uuid"
                      " AND position(:'email' IN lower(channel::text)) > 0 ORDER BY id",
  # pigeon_acl.entity_id holds org ids too, so an org id pasted into the
  # list would sweep away every grant that org confers.
  "org_identity": "SELECT count(*)::text FROM organizations WHERE id = :'id'::uuid",
}

# The same columns, as one count each -- the post-apply proof.
VERIFY_QUERIES = {
  "flocks.user_id": "SELECT count(*) FROM flocks WHERE user_id = :'id'::uuid",
  "flocks.owner_email": "SELECT count(*) FROM flocks WHERE lower(owner_email) = :'email'",
  "pigeon_acl.entity_id": "SELECT count(*) FROM pigeon_acl WHERE entity_id = :'id'::uuid",
  "alert_definitions.user_id": "SELECT count(*) FROM alert_definitions"
                               " WHERE user_id = :'id'::uuid",
  "organization_members.user_id": "SELECT count(*) FROM organization_members"
                                  " WHERE user_id = :'id'::uuid",
  "organization_members.invited_by": "SELECT count(*) FROM organization_members"
                                     " WHERE invited_by = :'id'::uuid",
  "organization_members.email": "SELECT count(*) FROM organization_members"
                                " WHERE lower(email) = :'email'",
  "organization_invites.created_by": "SELECT count(*) FROM organization_invites"
                                     " WHERE created_by = :'id'::uuid",
  "organization_invites.email": "SELECT count(*) FROM organization_invites"
                                " WHERE lower(email) = :'email'",
  "consent_events.identity_id": "SELECT count(*) FROM consent_events"
                                " WHERE identity_id = :'id'::uuid",
  "dashboard_state.user_id": "SELECT count(*) FROM dashboard_state WHERE user_id = :'id'::uuid",
  "error_events.user_id": "SELECT count(*) FROM error_events WHERE user_id = :'id'::uuid",
  "contact_submissions.user_id": "SELECT count(*) FROM contact_submissions"
                                 " WHERE user_id = :'id'::uuid",
  "billing_usage_periods.owner_id": "SELECT count(*) FROM billing_usage_periods"
                                    " WHERE owner_id = :'id'::uuid",
  "billing_usage_periods.owner_id (deleted orgs)": "SELECT count(*) FROM billing_usage_periods"
                                                   " WHERE owner_id = ANY(:'orgs'::uuid[])",
  "billing_meter_reports.org_id": "SELECT count(*) FROM billing_meter_reports"
                                  " WHERE org_id = ANY(:'orgs'::uuid[])",
  "organizations.id": "SELECT count(*) FROM organizations WHERE id = ANY(:'orgs'::uuid[])",
  # The gateway's Postgres sync is best-effort, so a 200 from the pigeon
  # route does not prove the mirror row went with the Durable Object.
  "pigeons.id (deleted pigeons)": "SELECT count(*) FROM pigeons"
                                  " WHERE id = ANY(:'pigeons'::text[])",
}


def inventory(db, identity):
  """Capture everything before a single delete: pigeon ids are DO addresses."""
  out = {"env": db.name}
  for key, sql in INVENTORY_QUERIES.items():
    rows = db.rows(sql, id=identity["id"], email=identity["email"] or NO_EMAIL)
    out[key] = int(rows[0][0]) \
        if key in ("errors", "contact", "usage", "owner_email", "org_identity") else rows
  return out


def is_empty(inv):
  return not any(inv[k] for k in INVENTORY_QUERIES)


# --- planning ----------------------------------------------------------


def plan_identity(identity, inventories, options):
  """Pure: an inventory per environment becomes an ordered step list or a hold.

  Order is not negotiable -- pigeons before flocks (a flock must be empty),
  flocks before orgs (an org must own none), and the identity last of all,
  because its session is the only key to its own Durable Objects.
  """
  steps, holds, notes = [], [], []
  deleted_orgs = {}
  empty_envs = []
  purging = {i.lower() for i in options.get("purging_ids", ())}
  for env, inv in sorted(inventories.items()):
    if env not in options["delete_envs"]:
      if not is_empty(inv):
        holds.append(f"rows in {env}, which this run did not select")
      continue
    if is_empty(inv):
      empty_envs.append(env)
    if inv["org_identity"]:
      holds.append(f"{env}: this id names an organization, not an identity -- a sweep keyed "
                   "on it would strip that org's grants from every pigeon it reaches")
      continue
    sole = {o[0] for o in inv["orgs"] if o[1] == "owner" and int(o[5]) == 1}
    # An org this identity solely owns goes with it, so a grant it confers
    # is not access anybody keeps.
    keeps_access = purging | {o.lower() for o in sole}
    for pigeon_id, _flock in inv["pigeons"]:
      steps.append({"env": env, "kind": "pigeon", "target": pigeon_id})
    for pigeon_id, entity_id, role in inv["acl_others"]:
      if entity_id.lower() in keeps_access:
        continue
      holds.append(f"{env}: pigeon {pigeon_id} also grants {role} to {entity_id}, which this "
                   "run keeps -- deleting it takes a device from an account that stays")
    for pigeon_id, role, own_flock in inv["acl"]:
      if own_flock != "t":
        # Someone else's device. The mirror row is swept below; the copy
        # inside that pigeon's Durable Object is reachable by no route.
        notes.append(f"{env}: {role} grant on surviving pigeon {pigeon_id} -- the Durable "
                     "Object's own copy survives; recreate that pigeon to clear it")
    org_flocks = {}
    for flock_id, _name, org_id, _pigeons in inv["flocks"]:
      if org_id and org_id not in sole:
        holds.append(f"{env}: flock {flock_id} belongs to org {org_id}, not solely theirs")
        continue
      steps.append({"env": env, "kind": "flock", "target": flock_id})
      if org_id:
        org_flocks[org_id] = org_flocks.get(org_id, 0) + 1
    for alert_id, _name in inv["alerts"]:
      steps.append({"env": env, "kind": "alert", "target": alert_id})
    for alert_id, owner in inv["alert_recipients"]:
      notes.append(f"{env}: alert {alert_id} of {owner} mails this address; the definition "
                   "survives and keeps mailing it")
    for org_id, other in inv["member_refs"]:
      notes.append(f"{env}: membership of {other} in org {org_id} names this address or this "
                   "inviter; the sweep blanks that reference")
    if inv["owner_email"]:
      notes.append(f"{env}: {inv['owner_email']} flock(s) owned by others carry this address "
                   "as owner_email; the sweep blanks it")
    if inv["errors"]:
      steps.append({"env": env, "kind": "errors", "target": ""})
    for (scope_key,) in inv["dashboard_state"]:
      steps.append({"env": env, "kind": "dashboard-state", "target": scope_key})
    org_steps, org_holds, org_notes, gone = plan_orgs(env, inv, options, identity["id"],
                                                      org_flocks)
    steps.extend(org_steps)
    holds.extend(org_holds)
    notes.extend(org_notes)
    deleted_orgs[env] = gone
  return {"id": identity["id"], "email": identity["email"], "steps": steps,
          "holds": holds, "notes": notes, "deleted_orgs": deleted_orgs,
          "empty_envs": empty_envs}


def plan_orgs(env, inv, options, identity_id, org_flocks):
  """An org is left, deleted, or reported -- never transferred by a script."""
  steps, holds, notes, gone = [], [], [], []
  for org_id, role, name, customer, subscription, members, _owners, flocks in inv["orgs"]:
    members, flocks = int(members), int(flocks)
    if role != "owner":
      steps.append({"env": env, "kind": "org-leave", "target": f"{org_id}/{identity_id}"})
      continue
    if members > 1:
      holds.append(f"{env}: sole owner of org {org_id} ({name!r}) with {members - 1} other "
                   "member(s) -- naming a successor is the owner's call, not a script's")
      continue
    if (customer or subscription) and not options["orphan_stripe"]:
      holds.append(f"{env}: org {org_id} ({name!r}) carries Stripe ids; "
                   "pass --orphan-stripe REASON once the subscription is cancelled")
      continue
    mine = org_flocks.get(org_id, 0)
    if flocks > mine:
      # A transferred flock keeps its creator's id as provenance, so the
      # identity's own rows do not name every flock the org owns -- and
      # DELETE /orgs refuses while any of them is left.
      holds.append(f"{env}: org {org_id} ({name!r}) owns {flocks} flock(s) but only {mine} "
                   "came from this identity; the rest need an owner before the org goes")
      continue
    for invite_id, invite_org, _email, spent in inv["invites"]:
      if invite_org == org_id and spent != "t":
        steps.append({"env": env, "kind": "invite", "target": f"{org_id}/{invite_id}"})
    steps.append({"env": env, "kind": "org-delete", "target": org_id})
    gone.append(org_id)
    if flocks:
      notes.append(f"{env}: org {org_id} owns {flocks} flock(s), deleted ahead of it")
  for invite_id, invite_org, _email, spent in inv["invites"]:
    if invite_org not in gone and spent != "t":
      holds.append(f"{env}: pending invite {invite_id} on surviving org {invite_org} -- "
                   "revoke it or let it expire before purging this account")
  return steps, holds, notes, gone


def sweep_sql(mode):
  """What no route in the product reaches. One transaction per database."""
  consent = "DELETE FROM consent_events WHERE identity_id = :'id'::uuid"
  if mode == "erasure":
    # Article 17(3)(e): the acceptance record is the evidence of the
    # contract, so it is the one purpose held back. Excluding it rather
    # than naming what goes means a purpose added later is erased.
    consent += f" AND purpose <> '{TERMS_PURPOSE}'"
  return [
    ("consent_events", consent),
    # Correspondence the sender addressed to us: detach, never delete.
    ("contact_submissions", "UPDATE contact_submissions SET user_id = NULL"
                            " WHERE user_id = :'id'::uuid"),
    ("billing_usage_periods (user)", "DELETE FROM billing_usage_periods"
                                     " WHERE owner_kind = 'user' AND owner_id = :'id'::uuid"),
    ("billing_usage_periods (org)", "DELETE FROM billing_usage_periods"
                                    " WHERE owner_kind = 'org'"
                                    " AND owner_id = ANY(:'orgs'::uuid[])"),
    ("billing_meter_reports", "DELETE FROM billing_meter_reports"
                              " WHERE org_id = ANY(:'orgs'::uuid[])"),
    ("organization_members", "DELETE FROM organization_members WHERE user_id = :'id'::uuid"),
    ("organization_members.invited_by", "UPDATE organization_members SET invited_by = NULL"
                                        " WHERE invited_by = :'id'::uuid"),
    ("organization_members.email", "UPDATE organization_members SET email = NULL"
                                   " WHERE lower(email) = :'email'"),
    # created_by is NOT NULL, so a spent invite row cannot be detached.
    ("organization_invites", "DELETE FROM organization_invites"
                             " WHERE created_by = :'id'::uuid OR lower(email) = :'email'"),
    ("pigeon_acl", "DELETE FROM pigeon_acl WHERE entity_id = :'id'::uuid"),
    ("error_events", "DELETE FROM error_events WHERE user_id = :'id'::uuid"),
    ("dashboard_state", "DELETE FROM dashboard_state WHERE user_id = :'id'::uuid"),
    ("flocks.owner_email", "UPDATE flocks SET owner_email = NULL"
                           " WHERE lower(owner_email) = :'email'"),
  ]


# --- the product's own routes ------------------------------------------


def api_delete(api, cookie, path):
  url = api + path
  return http("DELETE", url, headers={"Cookie": f"ory_kratos_session={cookie}"})[0]


def api_status(api, cookie, path):
  return http("GET", api + path, headers={"Cookie": f"ory_kratos_session={cookie}"})[0]


def step_path(step):
  kind, target = step["kind"], step["target"]
  if kind == "pigeon":
    return "/pigeons/" + urllib.parse.quote(target, safe="")
  if kind == "flock":
    return "/flocks/" + urllib.parse.quote(target, safe="")
  if kind == "alert":
    return "/alerts/" + urllib.parse.quote(target, safe="")
  if kind == "errors":
    return "/errors"
  if kind == "dashboard-state":
    return "/dashboard-state/" + urllib.parse.quote(target, safe="")
  if kind == "invite":
    org_id, _, invite_id = target.partition("/")
    return f"/orgs/{urllib.parse.quote(org_id, safe='')}/invites/" \
           f"{urllib.parse.quote(invite_id, safe='')}"
  if kind == "org-leave":
    org_id, _, user_id = target.partition("/")
    return f"/orgs/{urllib.parse.quote(org_id, safe='')}/members/" \
           f"{urllib.parse.quote(user_id, safe='')}"
  if kind == "org-delete":
    return "/orgs/" + urllib.parse.quote(target, safe="")
  raise ValueError(f"no path for step kind {kind!r}")


# How a step reads in the plan; anything unlisted is "delete <kind>".
STEP_VERBS = {
  "org-leave": "leave org",
  "org-delete": "delete org",
  "invite": "revoke invite",
  "errors": "delete error reports",
  "dashboard-state": "delete saved state",
}

def gone_target(step):
  """What GONE_CHECKS binds: the two composite targets carry an id each."""
  org_id, _, rest = step["target"].partition("/")
  if step["kind"] == "invite":
    return rest
  if step["kind"] == "org-leave":
    return org_id
  return step["target"]


GONE_CHECKS = {
  "flock": "SELECT count(*) FROM flocks WHERE id = :'t'::uuid",
  "alert": "SELECT count(*) FROM alert_definitions WHERE id = :'t'::uuid",
  "dashboard-state": "SELECT count(*) FROM dashboard_state"
                     " WHERE user_id = :'id'::uuid AND scope_key = :'t'",
  "invite": "SELECT count(*) FROM organization_invites WHERE id = :'t'::uuid",
  "org-delete": "SELECT count(*) FROM organizations WHERE id = :'t'::uuid",
  "errors": "SELECT count(*) FROM error_events WHERE user_id = :'id'::uuid",
  "org-leave": "SELECT count(*) FROM organization_members"
               " WHERE org_id = :'t'::uuid AND user_id = :'id'::uuid",
}


# --- run ---------------------------------------------------------------


class Run:
  def __init__(self, run_dir, resumed):
    self.dir = run_dir
    os.makedirs(os.path.join(run_dir, "inventory"), exist_ok=True)
    self.state_path = os.path.join(run_dir, "state.json")
    self.state = {}
    if resumed and os.path.exists(self.state_path):
      with open(self.state_path, encoding="utf-8") as fh:
        self.state = json.load(fh)
    self.log_path = os.path.join(run_dir, "log.ndjson")

  def done(self, identity_id, key):
    return key in self.state.get(identity_id, [])

  def mark(self, identity_id, key):
    self.state.setdefault(identity_id, [])
    if key not in self.state[identity_id]:
      self.state[identity_id].append(key)
    with open(self.state_path, "w", encoding="utf-8") as fh:
      json.dump(self.state, fh, indent=2)

  def log(self, **fields):
    fields["at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    with open(self.log_path, "a", encoding="utf-8") as fh:
      fh.write(json.dumps(fields) + "\n")


def confirm(expected):
  if not sys.stdin.isatty():
    die("--apply needs a terminal: it asks for the plan's ready count typed back")
  print(f"\nType the ready count ({expected}) to apply, anything else to abort: ", end="")
  sys.stdout.flush()
  if sys.stdin.readline().strip() != str(expected):
    die("count did not match; nothing was changed")


def purge_one(plan, identity, databases, kratos, run, options):
  """Objects through the product, then SQL, then the row check, then the identity.

  The counts have to be proved before the Kratos delete, not after: once
  the identity is gone there is no session left to finish anything they
  turn up. Returns the manual checks the run could not settle itself.
  """
  api_hosts = {env: ENVIRONMENTS[env][0] for env in options["delete_envs"]}
  manual = []
  pending = [s for s in plan["steps"]
             if not run.done(identity["id"], f"{s['env']}:{s['kind']}:{s['target']}")]
  # The session is only the key to the product's own routes; with nothing
  # left to call, minting one would fail on an identity already deleted.
  cookie = ""
  if pending:
    cookie = kratos.mint_session(identity["id"])
    run.log(identity=identity["id"], step="session", status="minted")

  for step in pending:
    key = f"{step['env']}:{step['kind']}:{step['target']}"
    db = databases[step["env"]]
    status = api_delete(api_hosts[step["env"]], cookie, step_path(step))
    outcome = "deleted"
    if status not in (200, 204):
      outcome = resolve_failure(step, status, db, identity, api_hosts, cookie)
      if outcome == "stop":
        run.log(identity=identity["id"], env=step["env"], step=step["kind"],
                target=step["target"], status=f"HTTP {status}")
        die(f"{step['kind']} {step['target']} in {step['env']} answered HTTP {status}; "
            "stopping this identity with its Durable Object still reachable")
      if outcome == "do-already-empty":
        # Now, not in the sweep: the flock delete that follows refuses
        # while a mirror row is still there.
        db.script("DELETE FROM pigeons WHERE id = :'t';", t=step["target"])
        manual.append(f"{step['env']}: pigeon {step['target']} answered {status} and its "
                      "access list was already empty -- read its Durable Object by hand")
    run.log(identity=identity["id"], env=step["env"], step=step["kind"],
            target=step["target"], status=outcome)
    run.mark(identity["id"], key)

  for env in options["delete_envs"]:
    db = databases[env]
    key = f"{env}:sweep"
    if run.done(identity["id"], key):
      continue
    if env in plan["empty_envs"]:
      # The inventory covers every column the sweep touches, so an empty
      # one means there is nothing to write.
      run.log(identity=identity["id"], env=env, step="sweep", status="nothing-to-sweep")
      run.mark(identity["id"], key)
      continue
    orgs = "{" + ",".join(plan["deleted_orgs"].get(env, [])) + "}"
    statements = "\n".join(sql + ";" for _label, sql in sweep_sql(options["mode"]))
    db.script(statements, id=identity["id"], email=identity["email"] or NO_EMAIL, orgs=orgs)
    run.log(identity=identity["id"], env=env, step="sweep", status="applied")
    run.mark(identity["id"], key)

  left = verify_rows(identity, plan, databases)
  if left:
    for line in left:
      print(f"    {line}")
    run.log(identity=identity["id"], step="verify", status="failed", rows=left)
    die(f"{identity['email'] or identity['id']}: rows survive the purge, so the identity is "
        "left in place -- its session is the only key to anything still keyed on it")
  run.log(identity=identity["id"], step="verify", status="clean")

  if not run.done(identity["id"], "kratos"):
    run.log(identity=identity["id"], step="revoke-sessions",
            status=kratos.revoke_sessions(identity["id"]))
    status = kratos.delete_identity(identity["id"])
    if status not in (204, 404):
      die(f"deleting identity {identity['id']} answered HTTP {status}")
    if kratos.identity_status(identity["id"]) != 404:
      die(f"identity {identity['id']} still resolves after deletion")
    run.log(identity=identity["id"], step="identity", status="deleted")
    run.mark(identity["id"], "kratos")

  if options["mode"] == "erasure":
    with open(options["dsar_log"], "a", encoding="utf-8") as fh:
      stamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
      fh.write(f"{stamp}\t{identity['id']}\t{identity['email']}\t{options['dsar_ref']}\n")

  return manual


def resolve_failure(step, status, db, identity, api_hosts, cookie):
  """A delete that did not answer 2xx: already gone, or a hard stop."""
  if step["kind"] == "pigeon":
    # An emptied Durable Object keeps no access-list row, so `detail`
    # answers 403 -- but so does a half-wiped one, a session that stopped
    # resolving, and an edge blip. Take only those two statuses, and only
    # while the same cookie still works on a route that must succeed.
    api = api_hosts[step["env"]]
    detail = api_status(api, cookie,
                        f"/pigeons/{urllib.parse.quote(step['target'], safe='')}/detail")
    if detail not in (403, 404) or api_status(api, cookie, "/orgs") != 200:
      return "stop"
    return "do-already-empty"
  check = GONE_CHECKS.get(step["kind"])
  if check and db.count(check, id=identity["id"], t=gone_target(step)) == 0:
    return "already-gone"
  return "stop"


def verify_rows(identity, plan, databases):
  """Direct psql only: a list route would answer from the query cache."""
  failures = []
  for env, db in sorted(databases.items()):
    orgs = "{" + ",".join(plan["deleted_orgs"].get(env, [])) + "}"
    pigeons = "{" + ",".join(s["target"] for s in plan["steps"]
                             if s["kind"] == "pigeon" and s["env"] == env) + "}"
    for label, sql in VERIFY_QUERIES.items():
      left = db.count(sql, id=identity["id"], email=identity["email"] or NO_EMAIL,
                      orgs=orgs, pigeons=pigeons)
      if left:
        failures.append(f"{env}: {left} row(s) left in {label}")
  return failures


def orphan_report(databases, known_ids):
  """Rows whose identity is in no inventory -- orphans that already exist."""
  ids = "{" + ",".join(sorted(known_ids)) + "}"
  columns = [
    ("flocks", "user_id"), ("pigeon_acl", "entity_id"), ("alert_definitions", "user_id"),
    ("organization_members", "user_id"), ("organization_invites", "created_by"),
    ("consent_events", "identity_id"), ("dashboard_state", "user_id"),
    ("error_events", "user_id"), ("contact_submissions", "user_id"),
  ]
  found = []
  for db in databases.values():
    for table, column in columns:
      # entity_id and friends hold org ids as well; an org is not an
      # identity that went missing.
      rows = db.rows(f"SELECT DISTINCT {column}::text FROM {table}"
                     f" WHERE NOT ({column} = ANY(:'ids'::uuid[]))"
                     f" AND {column} NOT IN (SELECT id FROM organizations)", ids=ids)
      for (value,) in rows:
        found.append(f"{db.name}: {table}.{column} = {value}")
  return found


# --- main --------------------------------------------------------------


def parse_args(argv):
  p = argparse.ArgumentParser(description="Purge fixture or erased accounts without orphans.")
  p.add_argument("--env", choices=["dev", "staging", "prod", "both"], required=True)
  source = p.add_mutually_exclusive_group(required=True)
  source.add_argument("--candidates-from", metavar="FILE",
                      help="a Kratos admin identity dump; the internal rule picks candidates")
  source.add_argument("--identities", metavar="FILE",
                      help="explicit ids or emails, one per line, '#' comments")
  p.add_argument("--keep", metavar="FILE", help="extra addresses or ids never to delete")
  p.add_argument("--dry-run", action="store_true")
  p.add_argument("--apply", action="store_true")
  p.add_argument("--include-held", action="store_true")
  p.add_argument("--orphan-stripe", metavar="REASON")
  p.add_argument("--allow-external", metavar="REASON")
  p.add_argument("--absent-ok", metavar="REASON",
                 help="sweep an id Kratos no longer knows; check it is not an org id")
  p.add_argument("--mode", choices=["fixture", "erasure"], default="fixture")
  p.add_argument("--dsar-log", metavar="PATH")
  p.add_argument("--dsar-ref", metavar="REF", default="")
  p.add_argument("--run-dir", metavar="PATH")
  p.add_argument("--resume", metavar="RUNDIR")
  return p.parse_args(argv)


def check_ids(rows):
  """Every id reaches psql as a -v binding; a bare UUID is the only shape allowed."""
  for row in rows:
    if not UUID_RE.match(row["id"]):
      die("an identity id is not a bare UUID; refusing to build SQL from it")
  return rows


def resolve_targets(args, kratos, keeps, held):
  """Returns (candidates, counts). Either the classified dump or an explicit list."""
  if args.candidates_from:
    identities = load_identities(args.candidates_from)
    counts = {"external": 0, "keep": 0, "held": 0, "candidate": 0}
    chosen = []
    for identity in identities:
      klass = classify_email(identity["email"], keeps, held)
      counts[klass] += 1
      if klass == "candidate" or (klass == "held" and args.include_held):
        chosen.append(identity)
    if counts != EXPECTED_CLASSES:
      die(f"inventory no longer splits {EXPECTED_CLASSES} -- got {counts}; "
          "re-read identities.json before trusting this plan")
    return check_ids(chosen), counts, [i["id"] for i in identities]

  wanted = []
  with open(args.identities, encoding="utf-8") as fh:
    for line in fh:
      line = line.split("#", 1)[0].strip()
      if line:
        wanted.append(line.lower())
  by_id = {}
  by_email = {}
  for identity in kratos.all_identities():
    email = (identity.get("traits", {}).get("email") or "").strip().lower()
    row = {"id": identity["id"], "email": email, "state": identity.get("state", "unknown")}
    by_id[identity["id"].lower()] = row
    by_email[email] = row
  chosen = []
  for want in wanted:
    row = by_id.get(want) or by_email.get(want)
    if not row and UUID_RE.match(want):
      # An id Kratos no longer knows may be an identity whose rows outlived
      # it -- or an organization id, which the sweep would read as one.
      if not args.absent_ok:
        die(f"{want} is a uuid Kratos does not know; pass --absent-ok REASON to sweep an "
            "identity that is already gone, and check first that it is not an org id")
      row = {"id": want, "email": "", "state": "absent"}
    if not row:
      die(f"{want} is in neither the identity ids nor the addresses Kratos knows")
    chosen.append(row)
  counts = {"external": 0, "keep": 0, "held": 0, "candidate": 0}
  for row in chosen:
    counts["candidate" if row["state"] == "absent"
           else classify_email(row["email"], keeps, held)] += 1
  return check_ids(chosen), counts, [row["id"] for row in by_id.values()]


def main(argv=None):
  args = parse_args(sys.argv[1:] if argv is None else argv)
  repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
  apply = args.apply
  if args.dry_run and apply:
    die("--dry-run and --apply contradict each other; without --apply nothing is changed")

  if args.env == "both":
    delete_envs = ["staging", "prod"]
  else:
    delete_envs = [args.env]
  realm = ENVIRONMENTS[delete_envs[0]][2]
  # dev shares nothing with the deployed realm, and mixing them is how a
  # local test run would reach production.
  inventory_envs = ["dev"] if realm == "dev" else ["staging", "prod"]
  if args.mode == "erasure" and not (args.dsar_log and args.dsar_ref):
    die("--mode erasure needs --dsar-log PATH and --dsar-ref REF: the log line is the only "
        "surviving link between a retained row and the person who asked")
  if args.mode == "erasure":
    die("erasure mode is written but refused: docs/legal/privacy.md and the DPA's Annex II G.3 "
        "both still say the acceptance record is deleted with the account, so keeping it would "
        "contradict the published text. Move both documents first.")

  stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
  run_dir = args.resume or args.run_dir or f"/tmp/purge-identities-{stamp}"
  run = Run(run_dir, bool(args.resume))

  keeps = set(KEEP_EMAILS)
  if args.keep:
    with open(args.keep, encoding="utf-8") as fh:
      for line in fh:
        line = line.split("#", 1)[0].strip().lower()
        if line:
          keeps.add(line)

  kratos = Kratos(realm, repo_root, run_dir)
  candidates, counts, known_ids = resolve_targets(args, kratos, keeps, HELD_EMAILS)

  databases = {}
  for env in inventory_envs:
    _api, secret_name, _realm = ENVIRONMENTS[env]
    uri = read_secret(secret_name, repo_root)
    if not uri and env == "dev":
      uri = DEV_PG_FALLBACK
    if not uri:
      die(f"{secret_name} is not set; it is needed to prove {env} holds no rows")
    databases[env] = Database(env, uri)

  # Both deployed strings name the same managed instance, so a crossed
  # variable is a plausible typo and an unrecoverable one: the run would
  # sweep one database while calling it the other.
  opened = {}
  for env, db in databases.items():
    where = db.opened()
    if where in opened:
      die(f"{env} and {opened[where]} opened the same database ({where[0]}); check which "
          "connection string each variable holds")
    opened[where] = env

  # Every flag that waives a refusal is an authorization; the reason it
  # named is the record of who allowed what.
  reasons = {name: value for name, value in (("allow-external", args.allow_external),
                                             ("orphan-stripe", args.orphan_stripe),
                                             ("absent-ok", args.absent_ok)) if value}
  options = {
    "delete_envs": delete_envs,
    "mode": args.mode,
    "orphan_stripe": bool(args.orphan_stripe),
    "dsar_log": args.dsar_log,
    "dsar_ref": args.dsar_ref,
    # A held address is skipped below, so a grant to one is access that
    # stays rather than access this run takes with it.
    "purging_ids": {row["id"] for row in candidates
                    if args.include_held or row["email"] not in HELD_EMAILS},
  }
  run.log(event="start", env=args.env, mode=args.mode, apply=bool(apply), reasons=reasons)

  print(f"purge-identities: env={args.env} mode={args.mode} realm={realm}")
  print(f"  classes: {counts}")
  print(f"  keep list ({len(keeps)}): {', '.join(sorted(keeps))}")
  for email, reason in sorted(HELD_EMAILS.items()):
    state = "included by --include-held" if args.include_held else "held"
    print(f"  {state}: {email} -- {reason}")
  print(f"  deleting in: {', '.join(delete_envs)}; inventorying: {', '.join(inventory_envs)}")
  for where, env in sorted(opened.items(), key=lambda kv: kv[1]):
    print(f"  {env} database: {where[0]}")
  for name, reason in sorted(reasons.items()):
    print(f"  --{name}: {reason}")
  print(f"  run directory: {run_dir}\n")

  plans = []
  refused = []
  for identity in candidates:
    if identity.get("state") == "absent":
      print(f"  {identity['id']} is already gone from Kratos; sweeping its rows by id alone")
    klass = "candidate" if identity.get("state") == "absent" else \
        classify_email(identity["email"], keeps, HELD_EMAILS)
    if identity["id"].lower() in keeps:
      klass = "keep"
    if klass == "keep":
      die(f"{identity['email'] or identity['id']} is on the keep list; never deleted")
    if klass == "external" and not args.allow_external:
      die(f"{identity['email']} is not classed internal; pass --allow-external REASON "
          "only for a real erasure request")
    if klass == "held" and not args.include_held:
      refused.append(f"{identity['email']}: {HELD_EMAILS[identity['email']]}")
      continue
    inventories = {env: inventory(databases[env], identity) for env in inventory_envs}
    with open(os.path.join(run_dir, "inventory", identity["id"] + ".json"), "w",
              encoding="utf-8") as fh:
      json.dump({"identity": identity, "inventories": inventories}, fh, indent=2)
    plans.append(plan_identity(identity, inventories, options))

  ready = [p for p in plans if not p["holds"]]
  held = len(plans) - len(ready)

  for plan in plans:
    print(f"[{'HOLD' if plan['holds'] else 'plan'}] "
          f"{plan['email'] or '(no address; already gone from Kratos)'}  {plan['id']}")
    for step in plan["steps"]:
      verb = STEP_VERBS.get(step["kind"], "delete " + step["kind"])
      print(f"    {step['env']}: {verb} {step['target']}".rstrip())
    if not plan["steps"]:
      print("    (no objects; sweep and identity only)")
    for note in plan["notes"]:
      print(f"    - {note}")
    for hold in plan["holds"]:
      print(f"    ! {hold}")
  for line in refused:
    print(f"[skip] {line}")

  if args.candidates_from:
    orphans = orphan_report(databases, known_ids)
    print(f"\npre-existing orphans (no identity in the inventory): {len(orphans)}")
    for line in orphans:
      print(f"    {line}")

  stripe_ids = []
  for plan in plans:
    path = os.path.join(run_dir, "inventory", plan["id"] + ".json")
    with open(path, encoding="utf-8") as fh:
      captured = json.load(fh)["inventories"]
    for inv in captured.values():
      for org in inv["orgs"]:
        if org[3] or org[4]:
          stripe_ids.append(f"{plan['email']}\t{org[0]}\t{org[3]}\t{org[4]}")
  if stripe_ids:
    with open(os.path.join(run_dir, "stripe-to-cancel.txt"), "w", encoding="utf-8") as fh:
      fh.write("\n".join(stripe_ids) + "\n")
    print(f"\n{len(stripe_ids)} org(s) carry Stripe ids -- "
          f"cancel them yourself: {run_dir}/stripe-to-cancel.txt")

  with open(os.path.join(run_dir, "plan.json"), "w", encoding="utf-8") as fh:
    json.dump({"env": args.env, "mode": args.mode, "counts": counts, "reasons": reasons,
               "plans": plans}, fh, indent=2)

  print(f"\n{len(ready)} ready, {held} held, {len(refused)} skipped")
  if not apply:
    print("dry run: nothing was changed. Re-run with --apply to carry this out.")
    return 0

  confirm(len(ready))
  manual = []
  for plan in ready:
    identity = {"id": plan["id"], "email": plan["email"]}
    print(f"\npurging {plan['email'] or plan['id']} ...")
    manual.extend(f"{plan['email'] or plan['id']}: {line}"
                  for line in purge_one(plan, identity, databases, kratos, run, options))
    print("  verified: no rows in either database, identity 404")

  print(f"\napplied: {len(ready)} identity(ies)")
  if manual:
    print(f"{len(manual)} manual check(s) -- a Durable Object this run could not wipe itself:")
    for line in manual:
      print(f"    {line}")
  print(f"log: {run.log_path}")
  return 1 if manual else 0


if __name__ == "__main__":
  sys.exit(main())
