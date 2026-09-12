"""Read the runtime inventory with Testinfra and verify HTTP from the controller."""

from urllib.request import ProxyHandler, build_opener


testinfra_hosts = ["ansible://webservers"]


def test_cvd_context(host):
    cvd = host.ansible.get_variables()["cvd"]
    assert cvd["invocation_id"]


def test_greeting_http(host):
    variables = host.ansible.get_variables()
    http = build_opener(ProxyHandler({}))
    with http.open(variables["http_url"], timeout=5) as response:
        assert response.status == 200
        assert response.read().decode("utf-8") == variables["greeting"] + "\n"
