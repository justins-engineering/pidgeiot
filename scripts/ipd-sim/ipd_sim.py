#!/usr/bin/env python3
"""Simulated polling signs: our own stand-in for a sign that polls a web data source,
not any maker's software.

Every page and every answer says "simulated, from your 2015 reference". A simulated sign does
what the published documents say a sign does, and no more. This module reads a web data
source body the way the October 2025 LED manual's chapter 13 describes: flat or nested JSON in
dot notation, or <DataItems> XML.

  python3 -m unittest discover -s scripts/ipd-sim

Standard library only.
"""

import json
import xml.etree.ElementTree as ET

LABEL = "simulated, from your 2015 reference"


class PayloadError(ValueError):
  """A data source body that is not data items in the configured format."""


def scalar_text(value):
  """The text a data item receives for one JSON scalar.

  Numbers and booleans keep their JSON spelling. The manual does not say what a sign does with
  null, so null becomes empty text.
  """
  if isinstance(value, str):
    return value
  if value is None:
    return ""
  return json.dumps(value)


def flatten_json(value, name=""):
  """Every scalar in a JSON value as (dot-notation name, text), as the manual names them.

  Arrays index from 0, so a top-level array yields names such as "0.LocationName" and a nested
  one "0.EparkingTicketsPresent.1.TicketName" (manual 13.3).
  """
  if isinstance(value, dict):
    children = value.items()
  elif isinstance(value, list):
    children = ((str(i), v) for i, v in enumerate(value))
  else:
    return [(name, scalar_text(value))] if name else []
  pairs = []
  for key, child in children:
    pairs.extend(flatten_json(child, name + "." + key if name else key))
  return pairs


def parse_data_items_xml(text):
  """(name, value) for each <DataItem> of a <DataItems> document (manual 13.4).

  The manual warns that serving <DataRecs> is refused with a curl logic error, so any root but
  <DataItems> is refused here too.
  """
  try:
    root = ET.fromstring(text)
  except ET.ParseError as e:
    raise PayloadError("not XML: " + str(e)) from None
  if root.tag != "DataItems":
    raise PayloadError("root element is <" + root.tag + ">, not <DataItems>")
  pairs = []
  for item in root.iter("DataItem"):
    name = item.get("name")
    if name:
      pairs.append((name, item.text or ""))
  return pairs


def parse_payload(body, fmt):
  """(name, value) pairs from a data source body in the format the sign is configured for."""
  try:
    text = body.decode("utf-8")
  except UnicodeDecodeError:
    raise PayloadError("body is not UTF-8") from None
  if fmt == "xml":
    return parse_data_items_xml(text)
  try:
    value = json.loads(text)
  except ValueError as e:
    raise PayloadError("not JSON: " + str(e)) from None
  return flatten_json(value)
