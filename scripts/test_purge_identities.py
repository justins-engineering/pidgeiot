#!/usr/bin/env python3
"""Unit tests for the purge planner and the internal/external classification.

  python3 -m unittest discover -s scripts -p 'test_*.py'

Pure functions only: nothing here opens a socket, a database or a shell.
"""

import importlib.util
import json
import os
import tempfile
import unittest

_SPEC = importlib.util.spec_from_file_location(
  "purge_identities",
  os.path.join(os.path.dirname(os.path.abspath(__file__)), "purge-identities.py"))
purge = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(purge)

KEEPS = purge.KEEP_EMAILS
HELD = purge.HELD_EMAILS
OPTIONS = {"delete_envs": ["staging"], "mode": "fixture", "orphan_stripe": False,
           "purging_ids": set()}


def empty_inventory(env="staging", **overrides):
  inv = {"env": env, "flocks": [], "pigeons": [], "acl": [], "acl_others": [], "orgs": [],
         "alerts": [], "alert_recipients": [], "dashboard_state": [], "invites": [],
         "consent": [], "errors": 0, "contact": 0, "usage": 0, "member_refs": [],
         "owner_email": 0, "org_identity": 0}
  inv.update(overrides)
  return inv


def one_of_every_kind():
  """An inventory that makes the planner emit every step kind it knows."""
  return empty_inventory(
    flocks=[["f1", "Flock", "", "1"]], pigeons=[["p1", "f1"]], alerts=[["a1", "Alert"]],
    errors=1, dashboard_state=[["graphs.v1.f1"]],
    orgs=[org_row("o1"), org_row("o2", role="member", members=3)],
    invites=[["i1", "o1", "x@y", "f"]])


def org_row(org_id, role="owner", name="Org", customer="", subscription="",
            members=1, owners=1, flocks=0):
  return [org_id, role, name, customer, subscription, str(members), str(owners), str(flocks)]


class Classification(unittest.TestCase):
  def test_the_four_keeps_are_never_candidates(self):
    for email in KEEPS:
      self.assertEqual(purge.classify_email(email, KEEPS, HELD), "keep")

  def test_a_stranger_is_external(self):
    for email in ("someone@gmail.com", "helpdesk@pvta.com", "2166724122@qq.com"):
      self.assertEqual(purge.classify_email(email, KEEPS, HELD), "external")

  def test_the_mail_catcher_is_held_but_its_plus_addresses_are_not(self):
    self.assertEqual(purge.classify_email("staging-catch@pidgeiot.com", KEEPS, HELD), "held")
    self.assertEqual(
      purge.classify_email("staging-catch+alertdiag-f4d51c@pidgeiot.com", KEEPS, HELD),
      "candidate")

  def test_each_internal_pattern_on_its_own(self):
    for email in ("code+consent-verify-1@jes.contact", "someone@example.com",
                  "person+e2e-1@elsewhere.net", "test-run@elsewhere.net"):
      self.assertEqual(purge.classify_email(email, KEEPS, HELD), "candidate")

  def test_case_and_whitespace_do_not_change_the_class(self):
    self.assertEqual(purge.classify_email("  CODE@JES.contact ", KEEPS, HELD), "keep")

  def test_an_extra_keep_wins_over_the_internal_rule(self):
    keeps = set(KEEPS) | {"staging-catch+samples98-9fe947@pidgeiot.com"}
    self.assertEqual(
      purge.classify_email("staging-catch+samples98-9fe947@pidgeiot.com", keeps, HELD), "keep")

  def test_a_missing_address_is_external_rather_than_a_candidate(self):
    self.assertEqual(purge.classify_email("", KEEPS, HELD), "external")


class InventoryLoading(unittest.TestCase):
  def _write(self, text):
    handle = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
    handle.write(text)
    handle.close()
    self.addCleanup(os.unlink, handle.name)
    return handle.name

  def test_json_lines(self):
    path = self._write('{"id":"a","traits":{"email":"A@x"},"state":"active"}\n'
                       '{"id":"b","traits":{"email":"b@x"},"state":"active"}\n')
    self.assertEqual([i["email"] for i in purge.load_identities(path)], ["a@x", "b@x"])

  def test_concatenated_pages(self):
    page = json.dumps([{"id": "a", "traits": {"email": "a@x"}}])
    self.assertEqual(len(purge.load_identities(self._write(page + page))), 2)

  def test_a_traitless_identity_still_loads(self):
    self.assertEqual(purge.load_identities(self._write('{"id":"a"}'))[0]["email"], "")


class Planner(unittest.TestCase):
  identity = {"id": "11111111-1111-1111-1111-111111111111", "email": "fixture@jes.contact"}

  def plan(self, inv, options=None):
    return purge.plan_identity(self.identity, {"staging": inv}, options or OPTIONS)

  def kinds(self, plan):
    return [(s["kind"], s["target"]) for s in plan["steps"]]

  def test_nothing_owned_still_purges_the_identity(self):
    plan = self.plan(empty_inventory())
    self.assertEqual(plan["steps"], [])
    self.assertEqual(plan["holds"], [])
    self.assertEqual(plan["empty_envs"], ["staging"])

  def test_an_environment_holding_anything_is_not_marked_empty(self):
    plan = self.plan(empty_inventory(errors=1))
    self.assertEqual(plan["empty_envs"], [])

  def test_pigeons_are_deleted_before_their_flock(self):
    inv = empty_inventory(
      flocks=[["f1", "Flock", "", "2"]],
      pigeons=[["p1", "f1"], ["p2", "f1"]])
    self.assertEqual(self.kinds(self.plan(inv)),
                     [("pigeon", "p1"), ("pigeon", "p2"), ("flock", "f1")])

  def test_a_flock_is_deleted_before_the_org_that_owns_it(self):
    inv = empty_inventory(flocks=[["f1", "Flock", "o1", "0"]],
                          orgs=[org_row("o1", flocks=1)])
    self.assertEqual(self.kinds(self.plan(inv)), [("flock", "f1"), ("org-delete", "o1")])

  def test_an_org_with_another_member_is_held_not_transferred(self):
    plan = self.plan(empty_inventory(orgs=[org_row("o1", members=2)]))
    self.assertEqual(plan["steps"], [])
    self.assertIn("successor", plan["holds"][0])

  def test_membership_of_someone_elses_org_is_left_by_the_route_that_removes_a_member(self):
    plan = self.plan(empty_inventory(orgs=[org_row("o1", role="member", members=3)]))
    self.assertEqual(self.kinds(plan), [("org-leave", f"o1/{self.identity['id']}")])
    self.assertEqual(purge.step_path(plan["steps"][0]),
                     f"/orgs/o1/members/{self.identity['id']}")

  def test_an_org_owning_a_flock_the_identity_did_not_create_is_held(self):
    # A transferred flock keeps its creator's id, so the org owns one the
    # identity's own rows never name and DELETE /orgs would refuse.
    inv = empty_inventory(flocks=[["f1", "Flock", "o1", "0"]],
                          orgs=[org_row("o1", flocks=2)])
    plan = self.plan(inv)
    self.assertEqual(self.kinds(plan), [("flock", "f1")])
    self.assertTrue(any("only 1 came from this identity" in h for h in plan["holds"]))

  def test_an_org_owning_only_this_identitys_flocks_is_still_deleted(self):
    inv = empty_inventory(flocks=[["f1", "Flock", "o1", "0"]],
                          orgs=[org_row("o1", flocks=1)])
    self.assertEqual(self.kinds(self.plan(inv)), [("flock", "f1"), ("org-delete", "o1")])

  def test_a_grant_another_account_holds_on_a_doomed_pigeon_holds_the_purge(self):
    inv = empty_inventory(flocks=[["f1", "Flock", "", "1"]], pigeons=[["p1", "f1"]],
                          acl_others=[["p1", "9e1a", "member"]])
    plan = self.plan(inv)
    self.assertTrue(any("takes a device from an account that stays" in h
                        for h in plan["holds"]))

  def test_a_grant_held_by_another_identity_in_the_same_run_is_not_a_hold(self):
    inv = empty_inventory(flocks=[["f1", "Flock", "", "1"]], pigeons=[["p1", "f1"]],
                          acl_others=[["p1", "9E1A", "member"]])
    options = dict(OPTIONS, purging_ids={"9e1a"})
    self.assertEqual(self.plan(inv, options)["holds"], [])

  def test_the_org_grant_on_a_doomed_pigeon_is_not_a_hold_when_the_org_goes_too(self):
    inv = empty_inventory(flocks=[["f1", "Flock", "o1", "1"]], pigeons=[["p1", "f1"]],
                          acl_others=[["p1", "o1", "owner"]], orgs=[org_row("o1", flocks=1)])
    self.assertEqual(self.plan(inv)["holds"], [])

  def test_an_id_that_names_an_organization_is_held_before_anything_is_planned(self):
    plan = self.plan(empty_inventory(org_identity=1, flocks=[["f1", "F", "", "0"]]))
    self.assertEqual(plan["steps"], [])
    self.assertIn("names an organization", plan["holds"][0])

  def test_an_alert_of_another_account_mailing_this_address_is_reported(self):
    plan = self.plan(empty_inventory(alert_recipients=[["a9", "someone"]]))
    self.assertTrue(any("keeps mailing it" in n for n in plan["notes"]))

  def test_a_stripe_carrying_org_is_held_until_the_flag_names_a_reason(self):
    inv = empty_inventory(orgs=[org_row("o1", customer="cus_x", subscription="sub_x")])
    self.assertIn("Stripe", self.plan(inv)["holds"][0])
    allowed = dict(OPTIONS, orphan_stripe=True)
    self.assertEqual(self.kinds(self.plan(inv, allowed)), [("org-delete", "o1")])

  def test_pending_invites_are_revoked_before_their_org_goes(self):
    inv = empty_inventory(orgs=[org_row("o1")],
                          invites=[["i1", "o1", "x@y", "f"], ["i2", "o1", "z@y", "t"]])
    self.assertEqual(self.kinds(self.plan(inv)),
                     [("invite", "o1/i1"), ("org-delete", "o1")])

  def test_a_pending_invite_on_a_surviving_org_holds_the_identity(self):
    inv = empty_inventory(invites=[["i1", "o9", "x@y", "f"]])
    self.assertIn("surviving org o9", self.plan(inv)["holds"][0])

  def test_a_flock_of_an_org_that_is_not_solely_theirs_is_held(self):
    inv = empty_inventory(flocks=[["f1", "Flock", "o1", "0"]],
                          orgs=[org_row("o1", members=2)])
    self.assertEqual(self.kinds(self.plan(inv)), [])
    self.assertTrue(any("not solely theirs" in h for h in self.plan(inv)["holds"]))

  def test_error_reports_and_saved_graphs_each_get_a_step(self):
    inv = empty_inventory(errors=3, dashboard_state=[["graphs.v1.pigeon.p1"], ["graphs.v1.f"]])
    self.assertEqual(self.kinds(self.plan(inv)),
                     [("errors", ""), ("dashboard-state", "graphs.v1.pigeon.p1"),
                      ("dashboard-state", "graphs.v1.f")])

  def test_a_grant_on_someone_elses_pigeon_is_a_note_never_a_delete(self):
    plan = self.plan(empty_inventory(acl=[["p9", "owner", "f"]]))
    self.assertEqual(plan["steps"], [])
    self.assertIn("Durable Object", plan["notes"][0])

  def test_rows_in_an_unselected_environment_hold_the_identity(self):
    plans = purge.plan_identity(
      self.identity,
      {"staging": empty_inventory(), "prod": empty_inventory("prod", errors=1)},
      OPTIONS)
    self.assertEqual(plans["holds"], ["rows in prod, which this run did not select"])

  def test_an_empty_unselected_environment_is_not_a_hold(self):
    plans = purge.plan_identity(
      self.identity,
      {"staging": empty_inventory(), "prod": empty_inventory("prod")},
      OPTIONS)
    self.assertEqual(plans["holds"], [])


class Sweep(unittest.TestCase):
  def labels(self, mode):
    return {label: sql for label, sql in purge.sweep_sql(mode)}

  def test_fixture_mode_takes_both_consent_purposes(self):
    self.assertNotIn("purpose", self.labels("fixture")["consent_events"])

  def test_erasure_mode_keeps_the_terms_assent_and_erases_anything_else(self):
    sql = self.labels("erasure")["consent_events"]
    self.assertIn(f"purpose <> '{purge.TERMS_PURPOSE}'", sql)
    self.assertNotIn("marketing_emails", sql)

  def test_correspondence_is_detached_rather_than_deleted(self):
    self.assertTrue(self.labels("fixture")["contact_submissions"].startswith("UPDATE"))

  def test_every_footprint_table_is_swept_or_verified(self):
    swept = " ".join(sql for _label, sql in purge.sweep_sql("fixture"))
    verified = " ".join(purge.VERIFY_QUERIES.values())
    for table in ("consent_events", "contact_submissions", "billing_usage_periods",
                  "billing_meter_reports", "organization_members", "organization_invites",
                  "pigeon_acl", "error_events", "dashboard_state"):
      self.assertIn(table, swept, table)
    for table in ("flocks", "pigeon_acl", "alert_definitions", "organization_members",
                  "organization_invites", "consent_events", "dashboard_state",
                  "error_events", "contact_submissions", "billing_usage_periods"):
      self.assertIn(table, verified, table)


class Paths(unittest.TestCase):
  def test_every_kind_the_planner_emits_has_a_route(self):
    plan = purge.plan_identity({"id": "i1", "email": "f@jes.contact"},
                               {"staging": one_of_every_kind()}, OPTIONS)
    self.assertEqual({s["kind"] for s in plan["steps"]},
                     {"pigeon", "flock", "alert", "errors", "dashboard-state", "invite",
                      "org-delete", "org-leave"})
    for step in plan["steps"]:
      self.assertTrue(purge.step_path(step).startswith("/"), step)

  def test_each_step_kind_maps_to_its_documented_route(self):
    cases = {
      ("pigeon", "p1"): "/pigeons/p1",
      ("org-leave", "o1/u1"): "/orgs/o1/members/u1",
      ("flock", "f1"): "/flocks/f1",
      ("alert", "a1"): "/alerts/a1",
      ("errors", ""): "/errors",
      ("dashboard-state", "graphs.v1.pigeon.p1"): "/dashboard-state/graphs.v1.pigeon.p1",
      ("invite", "o1/i1"): "/orgs/o1/invites/i1",
      ("org-delete", "o1"): "/orgs/o1",
    }
    for (kind, target), path in cases.items():
      self.assertEqual(purge.step_path({"kind": kind, "target": target}), path)

  def test_a_slash_in_a_scope_key_cannot_escape_its_segment(self):
    self.assertEqual(purge.step_path({"kind": "dashboard-state", "target": "a/b"}),
                     "/dashboard-state/a%2Fb")


class GoneChecks(unittest.TestCase):
  def test_an_invite_is_checked_by_its_own_id_not_the_org_prefixed_target(self):
    self.assertEqual(purge.gone_target({"kind": "invite", "target": "o1/i1"}), "i1")

  def test_every_other_kind_checks_its_target_verbatim(self):
    self.assertEqual(purge.gone_target({"kind": "flock", "target": "f1"}), "f1")

  def test_a_leave_step_is_checked_by_the_org_it_names_not_the_user(self):
    self.assertEqual(purge.gone_target({"kind": "org-leave", "target": "o1/u1"}), "o1")

  def test_a_scope_key_holding_a_slash_is_checked_whole(self):
    self.assertEqual(purge.gone_target({"kind": "dashboard-state", "target": "a/b"}), "a/b")

  def test_each_check_covers_a_kind_the_planner_can_emit(self):
    for kind in purge.GONE_CHECKS:
      self.assertIn(kind, {"flock", "alert", "dashboard-state", "invite", "org-delete",
                           "errors", "org-leave"})


class RemoteShell(unittest.TestCase):
  """The generated ssh script, checked as text -- nothing here runs a shell."""

  def script(self, body=None):
    return purge.Kratos.remote_script(
      purge.Kratos.__new__(purge.Kratos), "POST" if body else "GET",
      "http://127.0.0.1:4434/self-service/recovery?flow=f&token=t", body, True)

  def test_the_body_is_a_file_curl_reads_not_the_pipe_the_script_arrives_on(self):
    script = self.script('{"identity_id": "x"}')
    self.assertIn('cat >"$d" <<\'PURGE_BODY\'\n{"identity_id": "x"}\nPURGE_BODY', script)
    self.assertIn('--data-binary @"$d"', script)
    self.assertNotIn("--data-binary @-", script)

  def test_a_heredoc_never_follows_the_command_substitution_that_would_swallow_it(self):
    for line in self.script('{"a": 1}').splitlines():
      if line.startswith("code=$("):
        self.assertNotIn("<<", line)

  def test_the_one_time_token_reaches_curl_in_a_config_file_not_in_argv(self):
    script = self.script()
    self.assertIn('url = "http://127.0.0.1:4434/self-service/recovery?flow=f&token=t"', script)
    for line in script.splitlines():
      if line.startswith("code=$("):
        self.assertNotIn("token=t", line)

  def test_the_reply_is_framed_so_a_silent_remote_shell_cannot_read_as_a_status(self):
    script = self.script()
    self.assertIn("--headers--", script)
    self.assertIn("--body--", script)


class FailureResolution(unittest.TestCase):
  """A pigeon delete that did not answer 2xx. `api_status` is stubbed."""

  step = {"env": "staging", "kind": "pigeon", "target": "p1"}

  def answers(self, detail, orgs):
    seen = []

    def stub(_api, _cookie, path):
      seen.append(path)
      return detail if "/detail" in path else orgs

    original = purge.api_status
    purge.api_status = stub
    self.addCleanup(setattr, purge, "api_status", original)
    outcome = purge.resolve_failure(self.step, 500, None, {"id": "i1"},
                                    {"staging": "http://api"}, "cookie")
    return outcome, seen

  def test_an_empty_access_list_with_the_session_still_working_is_believed(self):
    self.assertEqual(self.answers(403, 200)[0], "do-already-empty")

  def test_an_expired_session_is_not_proof_that_a_durable_object_was_wiped(self):
    self.assertEqual(self.answers(401, 401)[0], "stop")

  def test_a_server_error_on_the_probe_is_not_proof_either(self):
    self.assertEqual(self.answers(500, 200)[0], "stop")

  def test_a_session_that_stopped_working_after_the_probe_stops_the_run(self):
    self.assertEqual(self.answers(403, 401)[0], "stop")


class Bindings(unittest.TestCase):
  def test_every_verify_query_uses_only_the_four_bindings_verify_rows_passes(self):
    import re
    for label, sql in purge.VERIFY_QUERIES.items():
      for name in re.findall(r":\'([a-z_]+)\'", sql):
        self.assertIn(name, ("id", "email", "orgs", "pigeons"), label)

  def test_the_sweep_no_longer_binds_a_pigeon_list_it_is_not_given(self):
    for label, sql in purge.sweep_sql("fixture"):
      self.assertNotIn(":\'pigeons\'", sql, label)


class ConnectionStrings(unittest.TestCase):
  def test_the_dev_string_decomposes_into_pg_variables(self):
    env = purge.pg_env(purge.DEV_PG_FALLBACK)
    self.assertEqual(env["PGHOST"], "127.0.0.1")
    self.assertEqual(env["PGDATABASE"], "dovecote")
    self.assertEqual(env["PGSSLMODE"], "disable")

  def test_a_percent_encoded_password_is_decoded(self):
    env = purge.pg_env("postgres://u:p%40ss@h:5433/d?sslmode=require")
    self.assertEqual(env["PGPASSWORD"], "p@ss")
    self.assertEqual(env["PGPORT"], "5433")


if __name__ == "__main__":
  unittest.main()
