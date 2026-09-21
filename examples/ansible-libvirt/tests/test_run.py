"""Check a completed real run: set CVD_STATE_FILE to its state.json path."""

import json
import os
import unittest
from collections import Counter
from pathlib import Path


class CompletedRunTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        filename = os.environ.get("CVD_STATE_FILE")
        if not filename:
            raise unittest.SkipTest("set CVD_STATE_FILE after a complete integration run")
        cls.state = json.loads(Path(filename).read_text())

    def test_reboots_and_verifications_follow_the_nested_lifecycle(self):
        self.assertFalse(self.state.get("primary_error"))
        self.assertFalse(self.state.get("cleanup_errors"))
        paths = [
            "etcd",
            "etcd/after_etcd1",
            "etcd/after_etcd1/after_etcd2",
            "etcd/after_etcd1/after_etcd2/after_etcd3",
        ]
        self.assertEqual(set(self.state["scenarios"]), set(paths))
        previous_reboot = None
        selected = self.state.get("requested_scenario")
        for path in paths:
            scenario = self.state["scenarios"][path]
            phases = scenario["phases"]
            ancestor = selected and selected.startswith(path + "/")
            if ancestor:
                self.assertEqual(phases["verify"]["status"], "skipped")
            else:
                self.assertEqual(phases["verify"]["status"], "pass")
                self.assertEqual(scenario["test_results"][0]["status"], "pass")
                if previous_reboot:
                    self.assertLessEqual(previous_reboot, phases["verify"]["started_at"])
            if path != paths[-1]:
                effect = phases["side_effect"]
                self.assertEqual(effect["status"], "pass")
                if previous_reboot:
                    self.assertLessEqual(previous_reboot, effect["started_at"])
                previous_reboot = effect["completed_at"]
        self.assertLessEqual(
            self.state["scenarios"][paths[-1]]["phases"]["verify"]["completed_at"],
            self.state["scenarios"]["etcd"]["phases"]["destroy"]["started_at"],
        )

    def test_parent_owns_and_destroys_all_resources(self):
        root = self.state["scenarios"]["etcd"]
        self.assertEqual(root["phases"]["destroy"]["status"], "pass")
        resources = root["resources"]["resources"]
        self.assertEqual(
            Counter(resource["type"] for resource in resources),
            {"libvirt.domain": 3, "libvirt.volume": 7, "libvirt.pool": 1, "local.directory": 1},
        )
        self.assertEqual(len({resource["id"] for resource in resources}), 12)
        for resource in resources:
            self.assertFalse(resource["exists"])
            self.assertEqual(resource["created"], {"scenario_path": "etcd", "phase": "create"})
            self.assertEqual(resource["destroyed"], {"scenario_path": "etcd", "phase": "destroy"})
        for path, scenario in self.state["scenarios"].items():
            if path != "etcd":
                self.assertEqual(scenario["resources"]["resources"], [])


if __name__ == "__main__":
    unittest.main()
