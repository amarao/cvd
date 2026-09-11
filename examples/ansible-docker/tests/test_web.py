"""Testinfra reads CVD's ANSIBLE_INVENTORY through its Ansible backend."""


testinfra_hosts = ["ansible://webservers"]


def test_cvd_context(host):
    cvd = host.ansible.get_variables()["cvd"]
    assert cvd["invocation_id"]


def test_greeting(host):
    variables = host.ansible.get_variables()
    greeting = host.file("/tmp/cvd-greeting")
    assert greeting.is_file
    assert greeting.content_string == variables["greeting"] + "\n"
