#!/usr/bin/env python3
"""Offline regressions for the signed probe's independent task verification."""
import importlib.util
import json
from pathlib import Path
import sqlite3
import unittest

spec = importlib.util.spec_from_file_location("probe", Path(__file__).with_name("check-signed-desktop.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class VerificationTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.execute("CREATE TABLE tasks(project_id TEXT,title TEXT,state TEXT,task_json TEXT)")

    def insert(self, project="project", state="Backlog", github=None):
        self.db.execute("INSERT INTO tasks VALUES(?,?,?,?)", (
            project, "unique-marker", json.dumps(state), json.dumps({"state": state, "github_source": github})))

    def test_requires_actual_task_not_assistant_claim(self):
        self.assertFalse(probe.verified_backlog_task(self.db, "project", "unique-marker"))
        self.insert()
        self.assertTrue(probe.verified_backlog_task(self.db, "project", "unique-marker"))

    def test_rejects_wrong_project_state_or_github(self):
        for kwargs in [{"project": "other"}, {"state": "Ready"}, {"github": {"number": 1}}]:
            with self.subTest(kwargs=kwargs):
                self.db.execute("DELETE FROM tasks")
                self.insert(**kwargs)
                self.assertFalse(probe.verified_backlog_task(self.db, "project", "unique-marker"))

    def test_rejects_duplicates(self):
        self.insert()
        self.insert()
        self.assertFalse(probe.verified_backlog_task(self.db, "project", "unique-marker"))

    def test_unwraps_native_results_but_does_not_treat_prose_as_receipt(self):
        output = {"content": [{"text": json.dumps({"ok": True, "result": {
            "snapshot_id": "s_fresh", "mode": "visual"}})}]}
        self.assertEqual(list(probe.tool_results(output)), [{"snapshot_id": "s_fresh", "mode": "visual"}])
        self.assertEqual(list(probe.tool_results("I clicked and created the task successfully")), [])


if __name__ == "__main__":
    unittest.main()
