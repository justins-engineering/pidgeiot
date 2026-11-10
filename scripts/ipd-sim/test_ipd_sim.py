#!/usr/bin/env python3
"""Unit tests for the simulated polling sign.

  python3 -m unittest discover -s scripts/ipd-sim

Sockets open only on 127.0.0.1 with ports the kernel picks.
"""

import unittest

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


if __name__ == "__main__":
  unittest.main()
