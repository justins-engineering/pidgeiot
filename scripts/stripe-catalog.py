#!/usr/bin/env python3
"""Build or check the PidgeIoT billing catalog in one Stripe environment.

dovecote resolves everything it bills through lookup keys and meter event
names, never pinned ids, so a fresh Stripe environment (a new sandbox, or
live mode at go-live) needs exactly this set and nothing else:

  products  PidgeIoT Builder|Growth|Scale|Fleet Plan, PidgeIoT usage
            tax_code txcd_10103001 (SaaS, business use)
  meters    overage_messages, extra_devices  (sum, by_id on stripe_customer_id)
  prices    builder|growth|scale|fleet        licensed, monthly
            message-overage                   metered on overage_messages
            device-overage-<tier>             metered on extra_devices
            all nine tax_behavior=exclusive

Stripe Tax needs tax_behavior on every price and a tax code on every
product before it computes correctly, and a price's tax_behavior can only
be set while it is still unspecified. Creating the catalog with both in
place is what makes a rebuild right the first time.

Additive only. Anything that already exists (a price by lookup key, a
meter by event name, a product by name) is reported, compared, and left
exactly as it is -- this never updates, archives or deletes a Stripe
object. Fixing an existing price's tax_behavior is a deliberate,
one-shot POST the owner runs by hand.

Usage:
  STRIPE_SECRET_KEY=... scripts/stripe-catalog.py            # dry run: report
  STRIPE_SECRET_KEY=... scripts/stripe-catalog.py --apply    # create what is missing
  STRIPE_SECRET_KEY=... scripts/stripe-catalog.py --apply --live   # a live key

The key is read from the environment and passed to Stripe in a header;
it never appears in argv, in output, or on disk. A live-mode key is
refused unless --live is given.
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.stripe.com"
API_VERSION = "2026-07-29.dahlia"
TAX_CODE = "txcd_10103001"
TAX_BEHAVIOR = "exclusive"

PLAN_DESCRIPTION = (
  "Includes up to {devices} connected devices and a pooled monthly allowance of "
  "{messages} messages. Connected devices beyond your plan are billed as usage, and "
  "each one adds 30,000 messages to your pooled allowance. Messages beyond that "
  "allowance are billed as usage."
)

# (lookup_key, product name, description, unit cents, devices, messages)
TIERS = [
  ("builder", "PidgeIoT Builder Plan", 2900, "50", "1.5 million"),
  ("growth", "PidgeIoT Growth Plan", 9900, "250", "7.5 million"),
  ("scale", "PidgeIoT Scale Plan", 34900, "1,500", "45 million"),
  ("fleet", "PidgeIoT Fleet Plan", 149900, "10,000", "300 million"),
]

USAGE_PRODUCT = (
  "PidgeIoT usage",
  "Usage above your plan's included device and message allowance, billed monthly "
  "based on your metered device and message counts.",
)

METERS = [
  ("overage_messages", "Overage messages"),
  ("extra_devices", "Extra devices"),
]

# (lookup_key, meter event_name, unit_amount_decimal in cents as a string)
# Cents are literal strings on purpose: 0.55 * 100 is not 55 in floating
# point, and Stripe rejects the resulting 55.00000000000001.
METERED_PRICES = [
  ("message-overage", "overage_messages", "0.003"),
  ("device-overage-builder", "extra_devices", "55"),
  ("device-overage-growth", "extra_devices", "35"),
  ("device-overage-scale", "extra_devices", "20"),
  ("device-overage-fleet", "extra_devices", "12"),
]


def die(msg):
  print(f"error: {msg}", file=sys.stderr)
  sys.exit(1)


class Stripe:
  def __init__(self, key):
    self._key = key

  def _call(self, method, path, params=None):
    url = API + path
    data = None
    headers = {
      "Authorization": f"Bearer {self._key}",
      "Stripe-Version": API_VERSION,
    }
    if params is not None:
      data = urllib.parse.urlencode(params).encode()
      headers["Content-Type"] = "application/x-www-form-urlencoded"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
      with urllib.request.urlopen(req) as resp:
        return json.load(resp)
    except urllib.error.HTTPError as e:
      body = e.read().decode(errors="replace")
      try:
        err = json.loads(body)["error"]
        detail = f"{err.get('type')} {err.get('code') or ''}: {err.get('message')}"
      except (ValueError, KeyError):
        detail = body[:300]
      die(f"{method} {path} -> HTTP {e.code} {detail}")

  def get_all(self, path, **query):
    query.setdefault("limit", 100)
    items = []
    starting_after = None
    while True:
      q = dict(query)
      if starting_after:
        q["starting_after"] = starting_after
      page = self._call("GET", path + "?" + urllib.parse.urlencode(q, doseq=True))
      items.extend(page["data"])
      if not page.get("has_more"):
        return items
      starting_after = items[-1]["id"]

  def post(self, path, params):
    return self._call("POST", path, params)


def main():
  apply = "--apply" in sys.argv
  live = "--live" in sys.argv
  key = os.environ.get("STRIPE_SECRET_KEY", "").strip()
  if not key:
    die("STRIPE_SECRET_KEY is not set in the environment")
  if "_live_" in key and not live:
    die("this is a live-mode key; pass --live to run against live mode")
  if "_test_" not in key and not live:
    die("key prefix is neither test nor live; refusing to guess")
  stripe = Stripe(key)
  del key

  problems = 0
  created = 0

  def note(kind, msg):
    nonlocal problems
    tag = {"ok": "  ok ", "mk": " new ", "warn": "WARN "}[kind]
    if kind == "warn":
      problems += 1
    print(f"{tag} {msg}")

  # Products, by name. Names are the only handle a product has that a
  # rebuild can predict; ids differ per environment.
  products = {p["name"]: p for p in stripe.get_all("/v1/products", active="true")}
  wanted_products = [(name, PLAN_DESCRIPTION.format(devices=d, messages=m))
                     for (_, name, _, d, m) in TIERS] + [USAGE_PRODUCT]
  for name, description in wanted_products:
    existing = products.get(name)
    if existing:
      note("ok", f"product {name!r} exists ({existing['id']})")
      if existing.get("tax_code") != TAX_CODE:
        note("warn", f"  tax_code is {existing.get('tax_code')!r}, catalog wants {TAX_CODE}")
      if (existing.get("description") or "") != description:
        note("warn", "  description differs from the catalog text (not changed)")
      continue
    note("mk", f"product {name!r}")
    if apply:
      products[name] = stripe.post("/v1/products", {
        "name": name,
        "description": description,
        "tax_code": TAX_CODE,
      })
      created += 1

  # Meters, by event name.
  meters = {m["event_name"]: m for m in stripe.get_all("/v1/billing/meters")
            if m.get("status") == "active"}
  for event_name, display_name in METERS:
    existing = meters.get(event_name)
    if existing:
      agg = existing.get("default_aggregation", {}).get("formula")
      mapping = existing.get("customer_mapping", {})
      note("ok", f"meter {event_name} exists ({existing['id']})")
      if agg != "sum" or mapping.get("event_payload_key") != "stripe_customer_id":
        note("warn", f"  aggregation {agg!r} / customer key "
                     f"{mapping.get('event_payload_key')!r} differ from what dovecote posts")
      continue
    note("mk", f"meter {event_name}")
    if apply:
      meters[event_name] = stripe.post("/v1/billing/meters", {
        "display_name": display_name,
        "event_name": event_name,
        "default_aggregation[formula]": "sum",
        "customer_mapping[type]": "by_id",
        "customer_mapping[event_payload_key]": "stripe_customer_id",
        "value_settings[event_payload_key]": "value",
      })
      created += 1

  # Prices, by lookup key -- the handle dovecote resolves at request time.
  lookup_keys = [t[0] for t in TIERS] + [p[0] for p in METERED_PRICES]
  prices = {p["lookup_key"]: p for p in stripe.get_all(
    "/v1/prices", active="true", **{"lookup_keys[]": lookup_keys})}

  def check_price(existing, key):
    note("ok", f"price {key} exists ({existing['id']})")
    if existing.get("tax_behavior") != TAX_BEHAVIOR:
      note("warn", f"  tax_behavior is {existing.get('tax_behavior')!r}; wants {TAX_BEHAVIOR} "
                   "(settable once while unspecified: POST /v1/prices/:id tax_behavior=exclusive)")

  for key, product_name, cents, _, _ in TIERS:
    existing = prices.get(key)
    if existing:
      check_price(existing, key)
      if existing.get("unit_amount") != cents:
        note("warn", f"  unit_amount is {existing.get('unit_amount')}, catalog says {cents}")
      continue
    product = products.get(product_name)
    note("mk", f"price {key} (${cents / 100:,.2f}/month on {product_name!r})")
    if apply:
      if not product:
        die(f"product {product_name!r} missing while creating price {key}")
      prices[key] = stripe.post("/v1/prices", {
        "product": product["id"],
        "lookup_key": key,
        "currency": "usd",
        "unit_amount": str(cents),
        "recurring[interval]": "month",
        "recurring[usage_type]": "licensed",
        "tax_behavior": TAX_BEHAVIOR,
      })
      created += 1

  usage_product = products.get(USAGE_PRODUCT[0])
  for key, event_name, cents in METERED_PRICES:
    existing = prices.get(key)
    if existing:
      check_price(existing, key)
      if existing.get("unit_amount_decimal") not in (cents, f"{float(cents):g}"):
        note("warn", f"  unit_amount_decimal is {existing.get('unit_amount_decimal')!r}, "
                     f"catalog says {cents!r}")
      meter = meters.get(event_name)
      if meter and existing.get("recurring", {}).get("meter") != meter["id"]:
        note("warn", f"  bound to meter {existing.get('recurring', {}).get('meter')!r}, "
                     f"not {event_name}")
      continue
    note("mk", f"price {key} ({cents} cents per unit, metered on {event_name})")
    if apply:
      meter = meters.get(event_name)
      if not usage_product or not meter:
        die(f"usage product or meter {event_name} missing while creating price {key}")
      prices[key] = stripe.post("/v1/prices", {
        "product": usage_product["id"],
        "lookup_key": key,
        "currency": "usd",
        "unit_amount_decimal": cents,
        "billing_scheme": "per_unit",
        "recurring[interval]": "month",
        "recurring[usage_type]": "metered",
        "recurring[meter]": meter["id"],
        "tax_behavior": TAX_BEHAVIOR,
      })
      created += 1

  # The acceptance check dovecote itself performs at request time.
  resolved = {p["lookup_key"] for p in stripe.get_all(
    "/v1/prices", active="true", **{"lookup_keys[]": lookup_keys})}
  missing = [k for k in lookup_keys if k not in resolved]
  if missing:
    note("warn", f"lookup keys not resolvable: {', '.join(missing)}"
                 + ("" if apply else " (dry run; --apply creates them)"))
  else:
    note("ok", f"all {len(lookup_keys)} lookup keys resolve")

  print()
  print(f"{'applied' if apply else 'dry run'}: {created} object(s) created, "
        f"{problems} thing(s) to look at")
  sys.exit(1 if problems else 0)


if __name__ == "__main__":
  main()
