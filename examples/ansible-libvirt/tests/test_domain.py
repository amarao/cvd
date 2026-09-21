"""Boot-device regression tests; run with python3 -m unittest discover -s tests."""

import unittest
from pathlib import Path
from xml.etree import ElementTree

import jinja2


SCENARIO = Path(__file__).resolve().parents[1]


class DomainBootTests(unittest.TestCase):
    def test_serial_only_guests_keep_a_vga_device_for_debian_grub(self):
        template = jinja2.Environment(
            loader=jinja2.FileSystemLoader(SCENARIO / "templates"),
            undefined=jinja2.StrictUndefined,
        ).get_template("domain.xml.j2")
        for acceleration in ("kvm", "qemu"):
            with self.subTest(acceleration=acceleration):
                domain = ElementTree.fromstring(
                    template.render(
                        libvirt_domain_type=acceleration,
                        libvirt_domain="test-etcd1",
                        libvirt_memory_mib=512,
                        libvirt_vcpus=1,
                        libvirt_root_volume={"create": {"path": "/test/root.qcow2"}},
                        libvirt_seed_volume={"create": {"path": "/test/seed.iso"}},
                        libvirt_network="default",
                    )
                )
                # Removing VGA made this Debian image reset immediately after
                # GRUB, even though serial console and the disk were correct.
                self.assertEqual(domain.find("devices/video/model").get("type"), "vga")
                self.assertIsNotNone(domain.find("devices/console"))
                self.assertIsNone(domain.find("devices/graphics"))
                self.assertEqual(domain.get("type"), acceleration)
                self.assertEqual(
                    domain.find("devices/disk[@device='disk']/driver").get("cache"),
                    "unsafe",
                )
                if acceleration == "kvm":
                    self.assertEqual(domain.find("cpu").get("mode"), "host-passthrough")
                else:
                    self.assertIsNone(domain.find("cpu"))


if __name__ == "__main__":
    unittest.main()
