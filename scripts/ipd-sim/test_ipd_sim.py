#!/usr/bin/env python3
"""Unit tests for the simulated polling sign.

  python3 -m unittest discover -s scripts/ipd-sim

Sockets open only on 127.0.0.1 with ports the kernel picks.
"""

import os
import tempfile
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
    xml = '<DataItems><DataItem name="rate">4</DataItem><DataItem name="no">5</DataItem></DataItems>'
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


if __name__ == "__main__":
  unittest.main()
