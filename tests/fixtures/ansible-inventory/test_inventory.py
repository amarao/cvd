import os
from pathlib import Path


testinfra_hosts = ["ansible://webservers"]


def test_effective_inventory(host):
    sources = os.environ["ANSIBLE_INVENTORY"].split(",")
    assert len(sources) == 2
    assert all(Path(source).is_absolute() for source in sources)
    assert Path(sources[-1]).name == "ansible-inventory.yml"
    variables = host.ansible.get_variables()
    assert variables["greeting"] == "from group_vars"
    if variables["inventory_hostname"] == "web":
        assert variables["ansible_host"] == "127.0.0.1"
        assert variables["host_setting"] == "from host_vars"
    assert host.run("/bin/true").rc == 0
