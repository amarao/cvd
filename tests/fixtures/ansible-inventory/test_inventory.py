import json
import os
from pathlib import Path

testinfra_hosts = ["ansible://webservers"]


def test_effective_inventory(host):
    sources = os.environ["ANSIBLE_INVENTORY"].split(",")
    assert len(sources) == 3
    assert all(Path(source).is_absolute() for source in sources)
    assert Path(sources[-2]).name == "ansible-inventory.yml"
    assert Path(sources[-1]).name == "cvd-inventory.yml"
    variables = host.ansible.get_variables()
    cvd = variables["cvd"]
    assert cvd["protocol_version"] == 1
    assert cvd["action"] == "verify"
    assert cvd["scenario_selector"] == "native"
    assert cvd["directory"] == os.environ["CVD_DIRECTORY"]
    assert cvd["vars"] == {}
    assert cvd["invocation_id"]
    assert json.loads(Path(cvd["input_file"]).read_text())["cvd"] == cvd
    assert not Path(cvd["result_file"]).exists()
    assert [resource["id"] for resource in cvd["resources"]] == ["local-web"]
    assert cvd["resources_by_type"]["test.local"] == cvd["resources"]
    assert cvd["resources"][0]["exists"] is True
    assert variables["greeting"] == "from group_vars"
    if variables["inventory_hostname"] == "web":
        assert variables["ansible_host"] == "127.0.0.1"
        assert variables["host_setting"] == "from host_vars"
    assert host.run("/bin/true").rc == 0
