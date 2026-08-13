from __future__ import annotations

import json
import os
from pathlib import Path

from ansible.plugins.callback import CallbackBase

DOCUMENTATION = r'''
name: cvd_resource_facts
short_description: Export CVD resource facts
description:
  - Records facts emitted by successful set_fact tasks as per-host JSON.
'''


class CallbackModule(CallbackBase):
    CALLBACK_VERSION = 2.0
    CALLBACK_TYPE = "aggregate"
    CALLBACK_NAME = "cvd_resource_facts"
    CALLBACK_NEEDS_ENABLED = True

    def __init__(self):
        super().__init__()
        self.facts = {}

    def v2_runner_on_ok(self, result):
        if result._task.action not in ("set_fact", "ansible.builtin.set_fact"):
            return
        returned = result._result.get("ansible_facts")
        if isinstance(returned, dict):
            host = result._host.get_name()
            self.facts.setdefault(host, {}).update(returned)

    def v2_playbook_on_stats(self, stats):
        target = Path(os.environ["CVD_RESOURCE_FACTS_FILE"])
        temporary = target.with_suffix(".tmp")
        with temporary.open("w", encoding="utf-8") as stream:
            json.dump(self.facts, stream)
        temporary.replace(target)
