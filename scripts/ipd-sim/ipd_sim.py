#!/usr/bin/env python3
"""Simulated polling signs: our own stand-in for a sign that polls a web data source,
not any maker's software.

Every page and every answer says "simulated, from your 2015 reference". A simulated sign does
what the published documents say a sign does, and no more:

- polls one web data source the way the October 2025 LED manual's chapter 13 describes (a GET
  at a set cadence, custom headers, Bearer or Basic auth) and applies flat or nested JSON in dot
  notation, or <DataItems> XML, keeping its last values when the source fails;
- reads and writes SOAP 1.1 under the names of the 2007 WSDL and the 2013 .NET client
  (rpc/encoded, SOAPAction http://tempuri.org/action/SignSvr.<name>);
- keeps a sign's layouts, messages and data items as XML files in the manual's chapter 21
  shape, with the 2015 reference's add-or-update, persist and reload rules.

  python3 -m unittest discover -s scripts/ipd-sim

Standard library only.
"""

import base64
import http.client
import json
import os
import ssl
import threading
import time
import urllib.parse
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


LAYOUTS_FILE = "layouts.xml"
MESSAGES_FILE = "messages.xml"
DATA_ITEMS_FILE = "dataitems.xml"
SIM_FILE = "sim-state.json"
PERSIST_ALL = 119
RELOAD_ALL = 106
FIRST_REC_ID = 3000
FIRST_TARGET_ID = 500


def int_attr(element, name):
  """An integer attribute, 0 when absent or not a number."""
  try:
    return int(element.get(name, "0"))
  except ValueError:
    return 0


def write_atomic(path, text):
  """Replace a file whole, so a crash never leaves half a table."""
  tmp = path + ".tmp"
  with open(tmp, "w", encoding="utf-8") as f:
    f.write(text)
  os.replace(tmp, path)


class SignState:
  """One sign's tables, in memory and in its state directory.

  The tables are XML in the shape of the LED manual's chapter 21: <Layouts> of <Layout> with
  <Zone> children (21.1), <Messages> of <Message> (21.2), <DataItems> of <DataItem> with a <Data>
  value (21.3). Layout and message edits stay in memory until SendCommand 119 (Persist All)
  writes them, and 106 (Reload All) or a restart reads the files back, so an unpersisted edit is
  lost. Data item values are written on every change, which is what keeps the last values across
  a restart; whether a real sign does the same is for the sample to show. Times the simulator
  adds (when each value last changed, the last poll) live beside the tables in sim-state.json.
  """

  def __init__(self, state_dir, data_item_names=()):
    os.makedirs(state_dir, exist_ok=True)
    self.dir = state_dir
    self.lock = threading.Lock()
    self._load_tables()
    sim = {}
    try:
      with open(self._path(SIM_FILE), encoding="utf-8") as f:
        sim = json.load(f)
    except (OSError, ValueError):
      pass
    self.changed_at = sim.get("changed_at", {})
    self.last_poll = sim.get("last_poll")
    self.last_ok_at = sim.get("last_ok_at")
    seeded = False
    for name in data_item_names:
      if self._data_item(name) is None:
        self.data_items.append(new_data_item(name))
        seeded = True
    if seeded:
      self._write_table(DATA_ITEMS_FILE, self.data_items)

  def _path(self, name):
    return os.path.join(self.dir, name)

  def _read_table(self, name, root_tag):
    try:
      root = ET.parse(self._path(name)).getroot()
    except (OSError, ET.ParseError):
      return ET.Element(root_tag)
    return root if root.tag == root_tag else ET.Element(root_tag)

  def _load_tables(self):
    self.layouts = self._read_table(LAYOUTS_FILE, "Layouts")
    self.messages = self._read_table(MESSAGES_FILE, "Messages")
    self.data_items = self._read_table(DATA_ITEMS_FILE, "DataItems")
    self.unsaved = False

  def _write_table(self, name, root):
    copy = ET.fromstring(ET.tostring(root))
    ET.indent(copy)
    write_atomic(self._path(name), ET.tostring(copy, encoding="unicode") + "\n")

  def _write_sim(self):
    write_atomic(self._path(SIM_FILE), json.dumps(
      {"changed_at": self.changed_at, "last_poll": self.last_poll, "last_ok_at": self.last_ok_at},
      indent=1) + "\n")

  def _data_item(self, name):
    for item in self.data_items.findall("DataItem"):
      if item.get("name") == name:
        return item
    return None

  def _next_rec_id(self):
    ids = [int_attr(e, "recID") for e in self.layouts.iter() if e.tag in ("Layout", "Zone")]
    ids += [int_attr(m, "id") for m in self.messages.findall("Message")]
    return max([FIRST_REC_ID] + ids) + 1

  def apply_values(self, pairs, now):
    """Set data item values by case-sensitive name, as the poll and UpdateDataItemValues do.

    A name the sign does not define is ignored, as the manual says a sign ignores it (13.2).
    Returns (number of defined items named, names ignored).
    """
    updated, ignored, changed = 0, [], False
    with self.lock:
      for name, value in pairs:
        item = self._data_item(name)
        if item is None:
          ignored.append(name)
          continue
        updated += 1
        data = item.find("Data")
        if data is None:
          data = ET.SubElement(item, "Data")
        if (data.text or "") != value:
          data.text = value
          self.changed_at[name] = now
          changed = True
      if changed:
        self._write_table(DATA_ITEMS_FILE, self.data_items)
        self._write_sim()
    return updated, ignored

  def update_data_item_values(self, xml_text, now):
    """UpdateDataItemValues: the number of data items updated, 0 for unreadable XML."""
    try:
      pairs = parse_data_items_xml(xml_text)
    except PayloadError:
      return 0
    return self.apply_values(pairs, now)[0]

  def record_poll(self, status):
    """Keep the last poll's outcome, and the time of the last one that succeeded."""
    with self.lock:
      self.last_poll = status
      if status.get("ok"):
        self.last_ok_at = status["at"]
      self._write_sim()

  def add_update_layout(self, xml_text):
    """AddUpdateLayout: the new or updated recID, 0 when the sign could not do it.

    A layout whose recID the sign holds replaces it; one without a recID is added with a new one.
    Zones take the layout's targetID as their parentID (manual 21.5).
    """
    try:
      layout = ET.fromstring(xml_text)
    except ET.ParseError:
      return 0
    if layout.tag != "Layout" or not layout.get("name"):
      return 0
    with self.lock:
      rec_id = int_attr(layout, "recID")
      old = [e for e in self.layouts.findall("Layout") if rec_id and int_attr(e, "recID") == rec_id]
      if not rec_id:
        rec_id = self._next_rec_id()
        layout.set("recID", str(rec_id))
      layout.set("parentID", layout.get("parentID", "0"))
      if not int_attr(layout, "targetID"):
        targets = [int_attr(e, "targetID") for e in self.layouts.findall("Layout")]
        layout.set("targetID", str(max([FIRST_TARGET_ID] + targets) + 1))
      next_id = max([self._next_rec_id(), rec_id + 1]
                    + [int_attr(z, "recID") + 1 for z in layout.findall("Zone")])
      for zone in layout.findall("Zone"):
        zone.set("parentID", layout.get("targetID"))
        if not int_attr(zone, "recID"):
          zone.set("recID", str(next_id))
          next_id += 1
      if old:
        index = list(self.layouts).index(old[0])
        self.layouts.remove(old[0])
        self.layouts.insert(index, layout)
      else:
        self.layouts.append(layout)
      self.unsaved = True
    return rec_id

  def add_update_message(self, xml_text):
    """AddUpdateMessage: the new or updated message id, 0 when the sign could not do it.

    A message is unique by its targetid and order, so a second one on the same pair replaces the
    first and keeps its id (2014 API manual, AddUpdateMessage).
    """
    try:
      message = ET.fromstring(xml_text)
    except ET.ParseError:
      return 0
    target, order = int_attr(message, "targetid"), int_attr(message, "order")
    if message.tag != "Message" or not target or not order:
      return 0
    with self.lock:
      old = [m for m in self.messages.findall("Message")
             if int_attr(m, "targetid") == target and int_attr(m, "order") == order]
      if old:
        message.set("id", old[0].get("id"))
        index = list(self.messages).index(old[0])
        self.messages.remove(old[0])
        self.messages.insert(index, message)
      else:
        message.set("id", str(self._next_rec_id()))
        self.messages.append(message)
      self.unsaved = True
      return int(message.get("id"))

  def send_command(self, command_id):
    """SendCommand: 1 when done, 0 for a command this simulator does not carry."""
    with self.lock:
      if command_id == PERSIST_ALL:
        self._write_table(LAYOUTS_FILE, self.layouts)
        self._write_table(MESSAGES_FILE, self.messages)
        self._write_table(DATA_ITEMS_FILE, self.data_items)
        self.unsaved = False
        return 1
      if command_id == RELOAD_ALL:
        self._load_tables()
        return 1
    return 0

  def snapshot(self):
    """What the status page and /state.json show."""
    with self.lock:
      return {
        "data_items": [{"name": i.get("name"), "value": i.findtext("Data") or "",
                        "changed_at": self.changed_at.get(i.get("name"))}
                       for i in self.data_items.findall("DataItem")],
        "layouts": [{"recID": l.get("recID"), "targetID": l.get("targetID"),
                     "name": l.get("name"), "zones": len(l.findall("Zone"))}
                    for l in self.layouts.findall("Layout")],
        "messages": [{"id": m.get("id"), "targetid": m.get("targetid"),
                      "order": m.get("order"),
                      "text": " ".join(t.text or "" for t in m.iter("Text"))}
                     for m in self.messages.findall("Message")],
        "unsaved": self.unsaved,
        "last_poll": self.last_poll,
        "last_ok_at": self.last_ok_at,
      }


def new_data_item(name):
  """A defined, empty data item in the manual's 21.3 shape."""
  item = ET.Element("DataItem", {"name": name, "type": "1", "align": "left", "width": "0",
                                 "category": ""})
  for child in ("Data", "InitValue", "Format", "Threshold"):
    ET.SubElement(item, child).text = ""
  return item


def sign_info_xml(name, height, width, mac, ip, now):
  """GetSignInfo's XML, shaped as the 2013 client's example.

  The example shows height, width, colors, macAddress, version, currentDate and currentTime. The
  2014 manual also lists the sign's name and IP address without naming their attributes, so
  "name" and "ipAddress" are this simulator's guesses.
  """
  t = time.localtime(now)
  hour = t.tm_hour % 12 or 12
  info = ET.Element("SignInfo", {
    "height": str(height), "width": str(width), "colors": "8", "macAddress": mac,
    "version": "simulated", "name": name, "ipAddress": ip,
    "currentDate": str(t.tm_year) + "-" + str(t.tm_mon) + "-" + str(t.tm_mday),
    "currentTime": str(hour) + ":" + str(t.tm_min).zfill(2) + ("AM" if t.tm_hour < 12 else "PM"),
  })
  ET.SubElement(info, "location").text = LABEL
  return ET.tostring(info, encoding="unicode")


MAX_BODY_BYTES = 1 << 20
FORMATS = ("json", "xml")
AUTHS = ("bearer", "basic", "none")
CREDENTIAL_KEYS = ("token", "password", "secret")
# The manual's NWS example sets this by hand (13.3); "" sends none, as libcurl does by default.
DEFAULT_USER_AGENT = "sign-sim/1.0"


class ConfigError(ValueError):
  """A configuration the simulator will not start with. Never carries a credential."""


def load_token(poll, environ):
  """The data source credential, from the environment variable or 0600 file the config names."""
  if poll["auth"] == "none":
    return None
  env, path = poll.get("token_env"), poll.get("token_file")
  if bool(env) == bool(path):
    raise ConfigError("name exactly one of token_env and token_file")
  if env:
    token = environ.get(env, "")
    if not token:
      raise ConfigError("environment variable " + env + " is unset or empty")
  else:
    try:
      mode = os.stat(path).st_mode
    except OSError as e:
      raise ConfigError("token file " + path + ": " + str(e.strerror)) from None
    if mode & 0o077:
      raise ConfigError("token file " + path + " must be mode 0600")
    with open(path, encoding="utf-8") as f:
      token = f.read()
  token = token.strip()
  if not token or "\n" in token or "\r" in token:
    raise ConfigError("token is empty or spans lines")
  return token


def poll_request(poll, token):
  """(split URL, request target, headers) for one data source GET.

  "params" is the manual's "Additional parameter(s)", appended to the URI as a query string.
  """
  url = urllib.parse.urlsplit(poll["url"])
  target = (url.path or "/") + ("?" + url.query if url.query else "")
  params = poll.get("params", "").lstrip("?&")
  if params:
    target += ("&" if "?" in target else "?") + params
  headers = {"Accept": "*/*"}
  if poll["user_agent"]:
    headers["User-Agent"] = poll["user_agent"]
  headers.update(poll.get("headers", {}))
  if poll["auth"] == "bearer":
    headers["Authorization"] = "Bearer " + token
  elif poll["auth"] == "basic":
    pair = (poll.get("username", "") + ":" + token).encode("utf-8")
    headers["Authorization"] = "Basic " + base64.b64encode(pair).decode("ascii")
  return url, target, headers


def http_get(poll, token, timeout):
  """(status, body) of one GET. Redirects are not followed, as curl does not by default."""
  url, target, headers = poll_request(poll, token)
  if url.scheme == "https":
    conn = http.client.HTTPSConnection(url.hostname, url.port, timeout=timeout,
                                       context=ssl.create_default_context())
  else:
    conn = http.client.HTTPConnection(url.hostname, url.port, timeout=timeout)
  try:
    conn.request("GET", target, headers=headers)
    response = conn.getresponse()
    return response.status, response.read(MAX_BODY_BYTES + 1)
  finally:
    conn.close()


def poll_once(poll, token, state, clock=time.time):
  """One data source request, recorded as the sign's last poll.

  Values change only on a 2xx body that parses. Anything else (unreachable, a non-2xx, a body
  that does not parse) leaves every data item at its last value.
  """
  status = {"at": clock(), "ok": False}
  try:
    code, body = http_get(poll, token, timeout=min(poll["every_secs"], 10))
  except (OSError, http.client.HTTPException) as e:
    status["error"] = ("unreachable: " + (str(e) or type(e).__name__))[:200]
  else:
    status["http"] = code
    if not 200 <= code < 300:
      status["error"] = "HTTP " + str(code)
    elif len(body) > MAX_BODY_BYTES:
      status["error"] = "body over " + str(MAX_BODY_BYTES) + " bytes"
    else:
      try:
        pairs = parse_payload(body, poll["format"])
      except PayloadError as e:
        status["error"] = str(e)[:200]
      else:
        updated, ignored = state.apply_values(pairs, status["at"])
        status.update(ok=True, updated=updated, ignored=ignored[:20], ignored_count=len(ignored))
  state.record_poll(status)
  return status


def _require(ok, text):
  if not ok:
    raise ConfigError(text)


def check_poll(poll, where):
  """Fill a poll section's defaults and refuse what the simulator cannot run."""
  _require(isinstance(poll, dict), where + ": poll must be an object")
  for key in CREDENTIAL_KEYS:
    _require(key not in poll, where + ": a credential never goes in the config; name token_env"
             " or token_file")
  url = urllib.parse.urlsplit(str(poll.get("url", "")))
  _require(url.scheme in ("http", "https") and url.hostname, where + ": url must be http(s)")
  poll.setdefault("format", "json")
  poll.setdefault("every_secs", 10)
  poll.setdefault("user_agent", DEFAULT_USER_AGENT)
  poll.setdefault("auth", "bearer")
  poll.setdefault("headers", {})
  _require(poll["format"] in FORMATS, where + ": format must be json or xml")
  _require(poll["auth"] in AUTHS, where + ": auth must be bearer, basic or none")
  _require(isinstance(poll["every_secs"], (int, float)) and poll["every_secs"] >= 1,
           where + ": every_secs must be at least 1")
  _require(isinstance(poll["user_agent"], str), where + ": user_agent must be text")
  _require(isinstance(poll["headers"], dict)
           and all(isinstance(v, str) for v in poll["headers"].values()),
           where + ": headers must map names to text")
  _require(all(k.lower() != "authorization" for k in poll["headers"]),
           where + ": Authorization comes from auth and the token, never from headers")


def load_config(path):
  """The fleet config, with defaults filled. It names credentials; it never holds one."""
  try:
    with open(path, encoding="utf-8") as f:
      config = json.load(f)
  except (OSError, ValueError) as e:
    raise ConfigError("config " + path + ": " + str(e)) from None
  _require(isinstance(config, dict) and isinstance(config.get("signs"), list)
           and config["signs"], "config needs a non-empty signs list")
  root = os.path.dirname(os.path.abspath(path))
  config["state_root"] = os.path.join(root, config.get("state_root", "ipd-sim-state"))
  config.setdefault("control_port", 8900)
  slugs, ports = set(), set()
  for sign in config["signs"]:
    _require(isinstance(sign, dict), "each sign must be an object")
    slug = str(sign.get("slug", ""))
    where = "sign " + (slug or "?")
    _require(slug and all(c.isascii() and (c.isalnum() or c == "-") for c in slug),
             where + ": slug must be letters, digits and hyphens")
    _require(slug not in slugs, where + ": slug is used twice")
    slugs.add(slug)
    sign.setdefault("name", slug)
    sign.setdefault("height", 32)
    sign.setdefault("width", 160)
    sign.setdefault("data_items", [])
    _require(isinstance(sign.get("port"), int) and 0 <= sign["port"] < 65536,
             where + ": port must be 0 to 65535")
    _require(sign["port"] == 0 or sign["port"] not in ports, where + ": port is used twice")
    ports.add(sign["port"])
    _require(all(isinstance(n, str) and n for n in sign["data_items"]),
             where + ": data_items must be names")
    check_poll(sign.get("poll"), where)
  return config
