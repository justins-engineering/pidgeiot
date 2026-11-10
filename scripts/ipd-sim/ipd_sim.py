#!/usr/bin/env python3
"""Simulated polling signs: our own stand-in for a sign that polls a web data source,
not any maker's software.

Every page and every answer says "simulated, from your 2015 reference". A simulated sign does
what the published documents say a sign does, and no more:

- reads a web data source body the way the October 2025 LED manual's chapter 13 describes: flat
  or nested JSON in dot notation, or <DataItems> XML;
- reads and writes SOAP 1.1 under the names of the 2007 WSDL and the 2013 .NET client
  (rpc/encoded, SOAPAction http://tempuri.org/action/SignSvr.<name>).

  python3 -m unittest discover -s scripts/ipd-sim

Standard library only.
"""

import json
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

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


# The 2007 WSDL in the public Quick Start zip binds every operation rpc/encoded, body namespace
# http://tempuri.org/message/, SOAPAction http://tempuri.org/action/SignSvr.<name>; the 2013
# .NET client uses the same action names.
SOAP_ENV = "http://schemas.xmlsoap.org/soap/envelope/"
SOAP_ENCODING = "http://schemas.xmlsoap.org/soap/encoding/"
MESSAGE_NS = "http://tempuri.org/message/"
ACTION_PREFIX = "http://tempuri.org/action/SignSvr."


class SoapError(ValueError):
  """A request that is not a SOAP 1.1 call this sign can read."""


def local_name(tag):
  """An element tag without its {namespace}."""
  return tag.rsplit("}", 1)[-1]


def parse_soap(body, soap_action):
  """(method, {part name: text}) from an rpc-style SOAP 1.1 request.

  SOAP 1.1 over HTTP requires the SOAPAction header, and the method it names must be the one the
  body calls.
  """
  if soap_action is None:
    raise SoapError("SOAPAction header missing")
  action = soap_action.strip().strip('"')
  if not action.startswith(ACTION_PREFIX):
    raise SoapError("SOAPAction is not " + ACTION_PREFIX + "<method>")
  method = action[len(ACTION_PREFIX):]
  try:
    root = ET.fromstring(body)
  except ET.ParseError as e:
    raise SoapError("not XML: " + str(e)) from None
  if root.tag != "{" + SOAP_ENV + "}Envelope":
    raise SoapError("not a SOAP 1.1 envelope")
  soap_body = root.find("{" + SOAP_ENV + "}Body")
  if soap_body is None or len(soap_body) == 0:
    raise SoapError("SOAP body is empty")
  call = soap_body[0]
  if local_name(call.tag) != method:
    raise SoapError("SOAPAction names " + method + " but the body calls " + local_name(call.tag))
  return method, {local_name(part.tag): part.text or "" for part in call}


def _envelope(inner):
  return ('<?xml version="1.0" encoding="UTF-8"?>'
          '<SOAP-ENV:Envelope xmlns:SOAP-ENV="' + SOAP_ENV + '"'
          ' xmlns:xsd="http://www.w3.org/2001/XMLSchema"'
          ' xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"'
          ' SOAP-ENV:encodingStyle="' + SOAP_ENCODING + '">'
          '<SOAP-ENV:Body>' + inner + '</SOAP-ENV:Body></SOAP-ENV:Envelope>').encode("utf-8")


def soap_response(method, parts):
  """An rpc/encoded <method>Response carrying (name, xsd type, value) parts in WSDL order."""
  inner = "".join('<' + name + ' xsi:type="xsd:' + xsd + '">' + escape(str(value)) + '</' + name
                  + '>' for name, xsd, value in parts)
  return _envelope('<m:' + method + 'Response xmlns:m="' + MESSAGE_NS + '">' + inner + '</m:'
                   + method + 'Response>')


def soap_fault(code, text):
  """A SOAP 1.1 fault; code is "Client" or "Server"."""
  return _envelope('<SOAP-ENV:Fault><faultcode>SOAP-ENV:' + code + '</faultcode><faultstring>'
                   + escape(text) + '</faultstring></SOAP-ENV:Fault>')
