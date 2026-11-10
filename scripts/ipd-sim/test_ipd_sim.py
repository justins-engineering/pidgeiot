#!/usr/bin/env python3
"""Unit tests for the simulated polling sign.

  python3 -m unittest discover -s scripts/ipd-sim

Sockets open only on 127.0.0.1 with ports the kernel picks.
"""

import base64
import http.client
import http.server
import json
import os
import tempfile
import threading
import time
import unittest
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

import ipd_sim


class FlattenJsonTest(unittest.TestCase):
  def test_flat_object_keeps_its_keys(self):
    pairs = ipd_sim.flatten_json({"rate": "2.50", "lot.status": "OPEN"})
    self.assertEqual(pairs, [("rate", "2.50"), ("lot.status", "OPEN")])

  def test_top_level_array_indexes_from_zero(self):
    # The manual's parking example, names as manual 13.3 lists them.
    body = [{"LocationName": "DFW Airport", "TransientAvailable": 2557},
            {"LocationName": "Denver International Airport", "TransientAvailable": 268}]
    pairs = dict(ipd_sim.flatten_json(body))
    self.assertEqual(pairs["0.LocationName"], "DFW Airport")
    self.assertEqual(pairs["0.TransientAvailable"], "2557")
    self.assertEqual(pairs["1.LocationName"], "Denver International Airport")

  def test_nested_objects_and_arrays_use_dot_notation(self):
    body = {"features": [{"properties": {"event": "Fire Weather Watch"}}],
            "title": "Current watches",
            "tickets": [[], [{"TicketName": "A1"}]]}
    pairs = dict(ipd_sim.flatten_json(body))
    self.assertEqual(pairs["features.0.properties.event"], "Fire Weather Watch")
    self.assertEqual(pairs["title"], "Current watches")
    self.assertEqual(pairs["tickets.1.0.TicketName"], "A1")
    self.assertNotIn("tickets.0", pairs)

  def test_scalars_keep_json_spelling_and_null_is_empty(self):
    pairs = dict(ipd_sim.flatten_json({"n": 12, "f": 1.5, "b": True, "z": None}))
    self.assertEqual(pairs, {"n": "12", "f": "1.5", "b": "true", "z": ""})

  def test_bare_scalar_names_nothing(self):
    self.assertEqual(ipd_sim.flatten_json(42), [])


class DataItemsXmlTest(unittest.TestCase):
  MANUAL_EXAMPLE = (b'<?xml version="1.0" encoding="UTF-8"?>\n<DataItems>\n'
                    b'  <DataItem name="lot.name">Greenville</DataItem>\n'
                    b'  <DataItem name="lot.count">224</DataItem>\n</DataItems>\n')

  def test_manual_example_parses(self):
    pairs = ipd_sim.parse_payload(self.MANUAL_EXAMPLE, "xml")
    self.assertEqual(pairs, [("lot.name", "Greenville"), ("lot.count", "224")])

  def test_datarecs_root_is_refused(self):
    with self.assertRaises(ipd_sim.PayloadError):
      ipd_sim.parse_payload(b'<DataRecs><DataRec name="a"/></DataRecs>', "xml")

  def test_broken_xml_is_refused(self):
    with self.assertRaises(ipd_sim.PayloadError):
      ipd_sim.parse_payload(b"<DataItems><DataItem", "xml")

  def test_empty_item_is_empty_text(self):
    self.assertEqual(ipd_sim.parse_payload(b'<DataItems><DataItem name="a"/></DataItems>', "xml"),
                     [("a", "")])


class PayloadTest(unittest.TestCase):
  def test_json_body(self):
    self.assertEqual(ipd_sim.parse_payload(b'{"rate":"3.00"}', "json"), [("rate", "3.00")])

  def test_broken_json_is_refused(self):
    with self.assertRaises(ipd_sim.PayloadError):
      ipd_sim.parse_payload(b'{"rate":', "json")

  def test_non_utf8_is_refused(self):
    with self.assertRaises(ipd_sim.PayloadError):
      ipd_sim.parse_payload(b'\xff\xfe', "json")


def soap_request(method, parts):
  """A request shaped as an rpc/encoded client such as the 2013 .NET one sends it."""
  inner = "".join('<' + name + ' xsi:type="xsd:' + xsd + '">' + escape(str(value)) + '</' + name
                  + '>' for name, xsd, value in parts)
  return ('<?xml version="1.0" encoding="utf-8"?>'
          '<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"'
          ' xmlns:soapenc="http://schemas.xmlsoap.org/soap/encoding/"'
          ' xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"'
          ' xmlns:xsd="http://www.w3.org/2001/XMLSchema">'
          '<soap:Body soap:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">'
          '<q1:' + method + ' xmlns:q1="http://tempuri.org/message/">' + inner + '</q1:' + method
          + '></soap:Body></soap:Envelope>').encode("utf-8")


def action(method):
  return '"http://tempuri.org/action/SignSvr.' + method + '"'


LAYOUT_XML = ('<Layout recID="3054" parentID="0" targetID="508" order="90" name="NewLayout2"'
              ' defcolor="00ff00" style="1" duration="75" top="0" bottom="32" left="0"'
              ' right="160" enabled="1"><Zone recID="3055" parentID="508" targetID="1" order="0"'
              ' name="NewLayout2-zone1" defcolor="00ff00" style="1" duration="0" top="0"'
              ' bottom="32" left="0" right="160" /></Layout>')


class SoapTest(unittest.TestCase):
  def test_request_parses_to_method_and_parts(self):
    body = soap_request("SendCommand", [("commandID", "int", 119), ("commandText", "string", "")])
    method, parts = ipd_sim.parse_soap(body, action("SendCommand"))
    self.assertEqual(method, "SendCommand")
    self.assertEqual(parts, {"commandID": "119", "commandText": ""})

  def test_escaped_xml_part_arrives_as_xml_text(self):
    body = soap_request("AddUpdateLayout", [("layoutXml", "string", LAYOUT_XML)])
    _, parts = ipd_sim.parse_soap(body, action("AddUpdateLayout"))
    self.assertEqual(parts["layoutXml"], LAYOUT_XML)

  def test_unquoted_action_is_accepted(self):
    body = soap_request("GetSignInfo", [])
    self.assertEqual(ipd_sim.parse_soap(body, "http://tempuri.org/action/SignSvr.GetSignInfo"),
                     ("GetSignInfo", {}))

  def test_missing_action_is_refused(self):
    with self.assertRaises(ipd_sim.SoapError):
      ipd_sim.parse_soap(soap_request("GetSignInfo", []), None)

  def test_action_and_body_must_agree(self):
    with self.assertRaises(ipd_sim.SoapError):
      ipd_sim.parse_soap(soap_request("GetSignInfo", []), action("SendCommand"))

  def test_foreign_action_is_refused(self):
    with self.assertRaises(ipd_sim.SoapError):
      ipd_sim.parse_soap(soap_request("GetSignInfo", []), '"urn:other#GetSignInfo"')

  def test_non_envelope_is_refused(self):
    with self.assertRaises(ipd_sim.SoapError):
      ipd_sim.parse_soap(b"<Envelope/>", action("GetSignInfo"))
    with self.assertRaises(ipd_sim.SoapError):
      ipd_sim.parse_soap(b"not xml", action("GetSignInfo"))

  def test_response_round_trips_with_escaped_xml(self):
    info = '<SignInfo height="32" width="160"><location>a &amp; b</location></SignInfo>'
    body = ipd_sim.soap_response("GetSignInfo", [("Result", "int", 1), ("signInfoXml", "string",
                                                                          info)])
    root = ET.fromstring(body)
    response = root.find("{http://schemas.xmlsoap.org/soap/envelope/}Body")[0]
    self.assertEqual(response.tag, "{http://tempuri.org/message/}GetSignInfoResponse")
    self.assertEqual([(p.tag, p.text) for p in response], [("Result", "1"), ("signInfoXml", info)])

  def test_fault_carries_its_text(self):
    root = ET.fromstring(ipd_sim.soap_fault("Client", "not simulated: <GetImages>"))
    fault = root.find("{http://schemas.xmlsoap.org/soap/envelope/}Body")[0]
    self.assertEqual(fault.findtext("faultcode"), "SOAP-ENV:Client")
    self.assertEqual(fault.findtext("faultstring"), "not simulated: <GetImages>")


MESSAGE_XML = ('<Message targetid="17" order="3" priority="3" timeout="15" scroll="0"'
               ' align="center" entry="2" exit="3" enabled="1"><Segments><Segment color="#FFFF00"'
               ' font="9B,Impact"><Text>9 HIGH FONT BOLD</Text></Segment></Segments></Message>')


class StateCase(unittest.TestCase):
  def setUp(self):
    self._tmp = tempfile.TemporaryDirectory()
    self.dir = os.path.join(self._tmp.name, "sign-a")

  def tearDown(self):
    self._tmp.cleanup()

  def state(self, names=("rate", "lot.status")):
    return ipd_sim.SignState(self.dir, names)


class DataItemStateTest(StateCase):
  def test_values_persist_across_a_restart(self):
    self.state().apply_values([("rate", "2.50"), ("lot.status", "OPEN")], 100.0)
    again = self.state()
    values = {i["name"]: (i["value"], i["changed_at"]) for i in again.snapshot()["data_items"]}
    self.assertEqual(values, {"rate": ("2.50", 100.0), "lot.status": ("OPEN", 100.0)})

  def test_file_is_chapter_21_shape(self):
    self.state().apply_values([("rate", "2.50")], 100.0)
    root = ET.parse(os.path.join(self.dir, "dataitems.xml")).getroot()
    self.assertEqual(root.tag, "DataItems")
    item = root.find("DataItem[@name='rate']")
    self.assertEqual(item.findtext("Data"), "2.50")
    self.assertEqual([c.tag for c in item], ["Data", "InitValue", "Format", "Threshold"])

  def test_changed_at_moves_only_when_the_value_changes(self):
    state = self.state()
    state.apply_values([("rate", "2.50")], 100.0)
    state.apply_values([("rate", "2.50")], 200.0)
    self.assertEqual(state.changed_at["rate"], 100.0)
    state.apply_values([("rate", "3.00")], 300.0)
    self.assertEqual(state.changed_at["rate"], 300.0)

  def test_undefined_names_are_ignored(self):
    updated, ignored = self.state().apply_values([("rate", "1"), ("Rate", "2"), ("x", "3")], 1.0)
    self.assertEqual((updated, ignored), (1, ["Rate", "x"]))

  def test_seeding_keeps_existing_values(self):
    self.state().apply_values([("rate", "2.50")], 100.0)
    again = self.state(("rate", "closure"))
    values = {i["name"]: i["value"] for i in again.snapshot()["data_items"]}
    self.assertEqual(values, {"rate": "2.50", "lot.status": "", "closure": ""})

  def test_update_data_item_values_counts_items_updated(self):
    xml = ('<DataItems><DataItem name="rate">4</DataItem><DataItem name="no">5</DataItem>'
           '</DataItems>')
    self.assertEqual(self.state().update_data_item_values(xml, 1.0), 1)
    self.assertEqual(self.state().update_data_item_values("<DataRecs/>", 1.0), 0)


class LayoutMessageStateTest(StateCase):
  def test_layout_without_persist_is_lost_on_restart(self):
    state = self.state()
    self.assertEqual(state.add_update_layout(LAYOUT_XML), 3054)
    self.assertTrue(state.snapshot()["unsaved"])
    self.assertEqual(self.state().snapshot()["layouts"], [])

  def test_persist_all_keeps_layout_and_message_across_restart(self):
    state = self.state()
    state.add_update_layout(LAYOUT_XML)
    message_id = state.add_update_message(MESSAGE_XML)
    self.assertEqual(state.send_command(ipd_sim.PERSIST_ALL), 1)
    self.assertFalse(state.snapshot()["unsaved"])
    again = self.state().snapshot()
    self.assertEqual(again["layouts"], [{"recID": "3054", "targetID": "508", "name": "NewLayout2",
                                         "zones": 1}])
    self.assertEqual(again["messages"], [{"id": str(message_id), "targetid": "17", "order": "3",
                                          "text": "9 HIGH FONT BOLD"}])
    root = ET.parse(os.path.join(self.dir, "layouts.xml")).getroot()
    self.assertEqual(root.find("Layout/Zone").get("parentID"), "508")

  def test_reload_all_discards_unpersisted_edits(self):
    state = self.state()
    state.add_update_layout(LAYOUT_XML)
    state.send_command(ipd_sim.PERSIST_ALL)
    state.add_update_layout(LAYOUT_XML.replace('name="NewLayout2"', 'name="Changed"'))
    self.assertEqual(state.snapshot()["layouts"][0]["name"], "Changed")
    self.assertEqual(state.send_command(ipd_sim.RELOAD_ALL), 1)
    self.assertEqual(state.snapshot()["layouts"][0]["name"], "NewLayout2")

  def test_same_rec_id_updates_in_place(self):
    state = self.state()
    state.add_update_layout(LAYOUT_XML)
    state.add_update_layout(LAYOUT_XML.replace('name="NewLayout2"', 'name="Changed"'))
    self.assertEqual([l["name"] for l in state.snapshot()["layouts"]], ["Changed"])

  def test_layout_without_ids_gets_new_ones(self):
    state = self.state()
    rec_id = state.add_update_layout(
      '<Layout name="L" order="1" duration="10"><Zone name="z1"/><Zone name="z2"/></Layout>')
    self.assertGreater(rec_id, ipd_sim.FIRST_REC_ID)
    layout = state.layouts.find("Layout")
    zone_ids = [z.get("recID") for z in layout.findall("Zone")]
    self.assertEqual(len(set(zone_ids + [str(rec_id)])), 3)
    self.assertEqual(layout.get("targetID"), str(ipd_sim.FIRST_TARGET_ID + 1))
    self.assertEqual({z.get("parentID") for z in layout.findall("Zone")}, {layout.get("targetID")})

  def test_unreadable_layout_returns_zero(self):
    state = self.state()
    self.assertEqual(state.add_update_layout("<Layout"), 0)
    self.assertEqual(state.add_update_layout('<Zone name="z"/>'), 0)
    self.assertEqual(state.add_update_layout('<Layout order="1"/>'), 0)

  def test_message_is_unique_by_target_and_order(self):
    state = self.state()
    first = state.add_update_message(MESSAGE_XML)
    second = state.add_update_message(MESSAGE_XML.replace('priority="3"', 'priority="1"'))
    other = state.add_update_message(MESSAGE_XML.replace('order="3"', 'order="4"'))
    self.assertEqual(first, second)
    self.assertNotEqual(first, other)
    self.assertEqual(len(state.snapshot()["messages"]), 2)
    self.assertEqual(state.add_update_message('<Message order="3"/>'), 0)

  def test_other_commands_are_not_carried(self):
    self.assertEqual(self.state().send_command(100), 0)

  def test_sign_info_names_the_simulation(self):
    info = ET.fromstring(ipd_sim.sign_info_xml("Lot A", 32, 160, "02aabbccddee", "127.0.0.1",
                                               time.mktime((2026, 11, 10, 18, 5, 0, 0, 0, -1))))
    self.assertEqual(info.get("currentDate"), "2026-11-10")
    self.assertEqual(info.get("currentTime"), "6:05PM")
    self.assertEqual(info.findtext("location"), ipd_sim.LABEL)
    self.assertEqual((info.get("height"), info.get("width")), ("32", "160"))


FAKE_TOKEN = "fake-test-token"


class FakeSource:
  """A data source on 127.0.0.1 that answers queued (status, body) pairs and keeps headers."""

  def __init__(self):
    source = self
    self.answers = []
    self.requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
      def do_GET(self):
        source.requests.append((self.path, dict(self.headers)))
        status, body = source.answers.pop(0) if source.answers else (404, b"")
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

      def log_message(self, *args):
        pass

    self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    self.port = self.server.server_address[1]
    self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
    self.thread.start()

  def close(self):
    self.server.shutdown()
    self.server.server_close()


def poll_section(port, **overrides):
  poll = {"url": "http://127.0.0.1:" + str(port) + "/device/pigeons/p1/values",
          "token_env": "IPD_SIM_TEST_TOKEN"}
  poll.update(overrides)
  ipd_sim.check_poll(poll, "test")
  return poll


class PollTest(StateCase):
  def setUp(self):
    super().setUp()
    self.source = FakeSource()
    self.addCleanup(self.source.close)

  def test_values_arrive_with_bearer_and_user_agent(self):
    self.source.answers.append((200, b'{"rate": "2.50", "unknown": 1}'))
    state = self.state()
    status = ipd_sim.poll_once(poll_section(self.source.port), FAKE_TOKEN, state)
    self.assertTrue(status["ok"])
    self.assertEqual((status["updated"], status["ignored"]), (1, ["unknown"]))
    path, headers = self.source.requests[0]
    self.assertEqual(path, "/device/pigeons/p1/values")
    self.assertEqual(headers["Authorization"], "Bearer " + FAKE_TOKEN)
    self.assertEqual(headers["User-Agent"], "sign-sim/1.0")
    self.assertEqual(state.snapshot()["data_items"][0]["value"], "2.50")

  def test_last_values_survive_every_kind_of_failure(self):
    state = self.state()
    poll = poll_section(self.source.port)
    self.source.answers.append((200, b'{"rate": "2.50", "lot.status": "OPEN"}'))
    first = ipd_sim.poll_once(poll, FAKE_TOKEN, state, clock=lambda: 100.0)
    self.source.answers += [(500, b"oops"), (401, b""), (200, b"<html>"), (200, b"[1,")]
    for _ in range(4):
      status = ipd_sim.poll_once(poll, FAKE_TOKEN, state, clock=lambda: 200.0)
      self.assertFalse(status["ok"])
    self.source.close()
    status = ipd_sim.poll_once(poll, FAKE_TOKEN, state, clock=lambda: 300.0)
    self.assertTrue(status["error"].startswith("unreachable"))
    snap = self.state().snapshot()
    self.assertEqual({i["name"]: (i["value"], i["changed_at"]) for i in snap["data_items"]},
                     {"rate": ("2.50", 100.0), "lot.status": ("OPEN", 100.0)})
    self.assertEqual(snap["last_ok_at"], first["at"])
    self.assertEqual(snap["last_poll"]["at"], 300.0)

  def test_xml_source(self):
    self.source.answers.append((200, b'<DataItems><DataItem name="rate">9</DataItem></DataItems>'))
    state = self.state()
    ipd_sim.poll_once(poll_section(self.source.port, format="xml"), FAKE_TOKEN, state)
    self.assertEqual(state.snapshot()["data_items"][0]["value"], "9")

  def test_basic_auth_params_and_no_user_agent(self):
    self.source.answers.append((200, b"{}"))
    poll = poll_section(self.source.port, auth="basic", username="sign", user_agent="",
                        params="?id=123", headers={"X-Site": "lot-a"})
    ipd_sim.poll_once(poll, FAKE_TOKEN, self.state())
    path, headers = self.source.requests[0]
    self.assertEqual(path, "/device/pigeons/p1/values?id=123")
    self.assertEqual(headers["Authorization"],
                     "Basic " + base64.b64encode(b"sign:" + FAKE_TOKEN.encode()).decode())
    self.assertNotIn("User-Agent", headers)
    self.assertEqual(headers["X-Site"], "lot-a")

  def test_token_never_reaches_the_state_files(self):
    self.source.answers.append((401, b"Bearer " + FAKE_TOKEN.encode()))
    ipd_sim.poll_once(poll_section(self.source.port), FAKE_TOKEN, self.state())
    for name in os.listdir(self.dir):
      with open(os.path.join(self.dir, name), encoding="utf-8") as f:
        self.assertNotIn(FAKE_TOKEN, f.read())


class TokenTest(unittest.TestCase):
  def setUp(self):
    self._tmp = tempfile.TemporaryDirectory()
    self.addCleanup(self._tmp.cleanup)

  def token_file(self, mode):
    path = os.path.join(self._tmp.name, "token")
    with open(path, "w", encoding="utf-8") as f:
      f.write(FAKE_TOKEN + "\n")
    os.chmod(path, mode)
    return path

  def test_environment_variable(self):
    poll = poll_section(1)
    self.assertEqual(ipd_sim.load_token(poll, {"IPD_SIM_TEST_TOKEN": FAKE_TOKEN}), FAKE_TOKEN)
    with self.assertRaises(ipd_sim.ConfigError):
      ipd_sim.load_token(poll, {})

  def test_file_must_be_0600(self):
    poll = poll_section(1, token_env=None, token_file=self.token_file(0o600))
    self.assertEqual(ipd_sim.load_token(poll, {}), FAKE_TOKEN)
    poll["token_file"] = self.token_file(0o644)
    with self.assertRaises(ipd_sim.ConfigError) as caught:
      ipd_sim.load_token(poll, {})
    self.assertNotIn(FAKE_TOKEN, str(caught.exception))

  def test_exactly_one_source(self):
    poll = poll_section(1, token_file=self.token_file(0o600))
    with self.assertRaises(ipd_sim.ConfigError):
      ipd_sim.load_token(poll, {"IPD_SIM_TEST_TOKEN": FAKE_TOKEN})
    self.assertIsNone(ipd_sim.load_token(poll_section(1, auth="none"), {}))

  def test_credential_in_config_is_refused(self):
    for key in ("token", "password"):
      with self.assertRaises(ipd_sim.ConfigError):
        poll_section(1, **{key: FAKE_TOKEN})
    with self.assertRaises(ipd_sim.ConfigError):
      poll_section(1, headers={"authorization": "Bearer x"})


class ConfigTest(unittest.TestCase):
  def write(self, config):
    tmp = tempfile.TemporaryDirectory()
    self.addCleanup(tmp.cleanup)
    path = os.path.join(tmp.name, "signs.json")
    with open(path, "w", encoding="utf-8") as f:
      json.dump(config, f)
    return path

  def sign(self, slug, port):
    return {"slug": slug, "port": port, "poll": {"url": "http://127.0.0.1:8787/x",
                                                 "token_env": "T"}}

  def test_defaults_fill_and_state_root_is_beside_the_config(self):
    path = self.write({"signs": [self.sign("sign-a", 8901)]})
    config = ipd_sim.load_config(path)
    sign = config["signs"][0]
    self.assertEqual(config["state_root"], os.path.join(os.path.dirname(path), "ipd-sim-state"))
    self.assertEqual((sign["name"], sign["poll"]["every_secs"], sign["poll"]["format"]),
                     ("sign-a", 10, "json"))

  def test_example_config_loads(self):
    here = os.path.dirname(os.path.abspath(__file__))
    config = ipd_sim.load_config(os.path.join(here, "signs.example.json"))
    self.assertEqual([s["port"] for s in config["signs"]], [8901, 8902, 8903])

  def test_duplicates_and_bad_values_are_refused(self):
    bad = [
      {"signs": [self.sign("a", 8901), self.sign("a", 8902)]},
      {"signs": [self.sign("a", 8901), self.sign("b", 8901)]},
      {"signs": [self.sign("a/b", 8901)]},
      {"signs": []},
      {"signs": [dict(self.sign("a", 8901), poll={"url": "ftp://x/", "token_env": "T"})]},
    ]
    for config in bad:
      with self.assertRaises(ipd_sim.ConfigError):
        ipd_sim.load_config(self.write(config))


def wait_for(predicate, secs=5.0):
  deadline = time.monotonic() + secs
  while time.monotonic() < deadline:
    if predicate():
      return True
    time.sleep(0.05)
  return False


class FleetTest(unittest.TestCase):
  """Two signs on kernel-picked ports, polling a fake source, driven over HTTP."""

  def setUp(self):
    self._tmp = tempfile.TemporaryDirectory()
    self.addCleanup(self._tmp.cleanup)
    quiet = ipd_sim.log
    ipd_sim.log = lambda text: None
    self.addCleanup(setattr, ipd_sim, "log", quiet)
    self.source = FakeSource()
    self.addCleanup(self.source.close)
    self.source.answers.append((200, b'{"rate": "<2.50>", "lot.status": "OPEN"}'))
    signs = []
    for slug in ("sign-a", "sign-b"):
      poll = poll_section(self.source.port, every_secs=1)
      signs.append({"slug": slug, "name": "Lot " + slug[-1].upper(), "port": 0, "height": 32,
                    "width": 160, "data_items": ["rate", "lot.status"], "poll": poll})
    config = {"signs": signs, "control_port": 0, "state_root": self._tmp.name}
    self.fleet = ipd_sim.Fleet(config, ipd_sim.DEFAULT_BIND, {"sign-a": FAKE_TOKEN,
                                                              "sign-b": FAKE_TOKEN})
    self.fleet.start()
    self.addCleanup(self.fleet.stop)
    self.a = self.fleet.signs["sign-a"]

  def request(self, port, method, path, body=None, headers=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
      conn.request(method, path, body=body, headers=headers or {})
      response = conn.getresponse()
      return response.status, response.read()
    finally:
      conn.close()

  def soap(self, method, parts, soap_action=None):
    headers = {"Content-Type": "text/xml; charset=utf-8",
               "SOAPAction": soap_action or action(method)}
    status, body = self.request(self.a.port, "POST", "/Soap1.wsdl", soap_request(method, parts),
                                headers)
    response = ET.fromstring(body).find("{http://schemas.xmlsoap.org/soap/envelope/}Body")[0]
    return status, {p.tag: p.text for p in response}

  def test_binds_loopback_only(self):
    self.assertEqual(self.a.server.server_address[0], "127.0.0.1")
    self.assertEqual(self.fleet.server.server_address[0], "127.0.0.1")

  def test_page_shows_label_and_polled_values(self):
    self.assertTrue(wait_for(lambda: any(i["value"] for i in self.a.status()["data_items"])))
    status, body = self.request(self.a.port, "GET", "/")
    page = body.decode("utf-8")
    self.assertEqual(status, 200)
    self.assertIn("Simulated, from your 2015 reference", page)
    self.assertIn("Lot A", page)
    self.assertIn("&lt;2.50&gt;", page)
    self.assertNotIn("<2.50>", page)
    self.assertNotIn(FAKE_TOKEN, page)
    _, state = self.request(self.a.port, "GET", "/state.json")
    self.assertEqual(json.loads(state)["label"], ipd_sim.LABEL)

  def test_soap_actions(self):
    status, parts = self.soap("AddUpdateLayout", [("layoutXml", "string", LAYOUT_XML)])
    self.assertEqual((status, parts["Result"]), (200, "3054"))
    _, parts = self.soap("AddUpdateMessage", [("messageXml", "string", MESSAGE_XML)])
    self.assertGreater(int(parts["Result"]), 0)
    values = '<DataItems><DataItem name="rate">7</DataItem></DataItems>'
    _, parts = self.soap("UpdateDataItemValues", [("dataItemValuesXml", "string", values)])
    self.assertEqual(parts["Result"], "1")
    for command in (119, 106):
      _, parts = self.soap("SendCommand", [("commandID", "int", command),
                                           ("commandText", "string", "")])
      self.assertEqual(parts["Result"], "1")
    _, parts = self.soap("GetSignInfo", [])
    self.assertEqual(parts["Result"], "1")
    info = ET.fromstring(parts["signInfoXml"])
    self.assertEqual((info.get("name"), info.findtext("location")), ("Lot A", ipd_sim.LABEL))

  def test_soap_refusals_are_faults(self):
    status, parts = self.soap("GetImages", [])
    self.assertEqual(status, 500)
    self.assertIn("not simulated", parts["faultstring"])
    status, parts = self.soap("GetSignInfo", [], soap_action=action("SendCommand"))
    self.assertEqual(status, 500)
    status, _ = self.request(self.a.port, "POST", "/Soap1.wsdl", soap_request("GetSignInfo", []))
    self.assertEqual(status, 500)

  def test_stop_and_start_one_sign(self):
    self.assertTrue(wait_for(lambda: any(i["value"] for i in self.a.status()["data_items"])))
    self.soap("AddUpdateLayout", [("layoutXml", "string", LAYOUT_XML)])
    self.soap("SendCommand", [("commandID", "int", 119), ("commandText", "string", "")])
    port, control = self.a.port, self.fleet.control_port
    status, _ = self.request(control, "POST", "/signs/sign-a/stop")
    self.assertEqual(status, 303)
    self.assertFalse(self.a.running)
    self.assertTrue(self.fleet.signs["sign-b"].running)
    with self.assertRaises(OSError):
      self.request(port, "GET", "/")
    status, body = self.request(control, "GET", "/")
    self.assertIn(b"stopped", body)
    status, _ = self.request(control, "POST", "/signs/sign-a/start")
    self.assertEqual(status, 303)
    snap = self.a.status()
    self.assertEqual({i["name"]: i["value"] for i in snap["data_items"]},
                     {"rate": "<2.50>", "lot.status": "OPEN"})
    self.assertEqual([l["recID"] for l in snap["layouts"]], ["3054"])

  def test_control_refuses_other_origins_and_unknown_signs(self):
    status, _ = self.request(self.fleet.control_port, "POST", "/signs/sign-a/stop",
                             headers={"Origin": "http://example.com"})
    self.assertEqual(status, 403)
    self.assertTrue(self.a.running)
    status, _ = self.request(self.fleet.control_port, "POST", "/signs/nope/stop")
    self.assertEqual(status, 404)


if __name__ == "__main__":
  unittest.main()
