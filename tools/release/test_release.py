"""Tests for release.py: `python3 tools/release/test_release.py`."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import release as r


class Versions(unittest.TestCase):
    def test_nightly_before_first_release(self):
        self.assertEqual(r.nightly_version("20261126", "abc1234def", []), "0.1.0-nightly.20261126+abc1234")

    def test_nightly_after_a_stable_release(self):
        tags = ["v0.1.0", "v0.1.1", "v0.2.0-rc.1", "v0.1.2"]
        self.assertEqual(r.nightly_version("20261126", "abc1234", tags), "0.2.0-nightly.20261126+abc1234")

    def test_tag_info(self):
        self.assertEqual(r.tag_info("v0.1.0")["latest"], "true")
        rc = r.tag_info("v0.2.0-rc.2")
        self.assertEqual((rc["channel"], rc["prerelease"], rc["latest"]), ("rc", "true", "false"))
        with self.assertRaises(SystemExit):
            r.tag_info("v0.2.1-beta.1")
        with self.assertRaises(SystemExit):
            r.tag_info("0.2.0")

    def test_archive_names(self):
        self.assertEqual(r.archive_name("0.1.0", "x86_64-linux"), "pelago-0.1.0-x86_64-linux.tar.gz")
        self.assertEqual(r.archive_name("0.1.0", "aarch64-windows"), "pelago-0.1.0-aarch64-windows.zip")


class Packaging(unittest.TestCase):
    def test_package_and_sums(self):
        with tempfile.TemporaryDirectory() as d:
            binary = os.path.join(d, "pelago")
            open(binary, "wb").write(b"\x7fELF")
            out = os.path.join(d, "out")
            for target in ("x86_64-linux", "aarch64-windows"):
                r.package(binary, target, "0.1.0", out)
            names = r.write_sums(out)
            self.assertEqual(len(names), 2)
            lines = open(os.path.join(out, "SHA256SUMS")).read().splitlines()
            self.assertEqual(len(lines), 2)
            self.assertTrue(all(len(l.split("  ")[0]) == 64 for l in lines))


class Pruning(unittest.TestCase):
    def test_keeps_newest_versions(self):
        names = []
        for day in range(1, 18):
            v = f"0.2.0-nightly.202611{day:02d}+abc{day:04d}"
            names += [f"pelago-{v}-x86_64-linux.tar.gz", f"SHA256SUMS-{v}", f"SHA256SUMS-{v}.sig"]
        drop = r.prune(names, 14)
        self.assertEqual(len(drop), 9)
        self.assertTrue(all("20261101" in n or "20261102" in n or "20261103" in n for n in drop))


class Packages(unittest.TestCase):
    def test_versions_sort_before_the_stable_release(self):
        v = r.package_versions("0.2.0-nightly.20261126+abc1234")
        self.assertEqual(v["deb"], "0.2.0~nightly.20261126.abc1234")
        self.assertEqual((v["rpm_version"], v["rpm_release"]), ("0.2.0", "0.nightly.20261126.abc1234"))
        v = r.package_versions("0.2.0")
        self.assertEqual((v["deb"], v["rpm_release"]), ("0.2.0", "1"))

    def test_deb_is_an_ar_archive_with_control_and_data(self):
        with tempfile.TemporaryDirectory() as d:
            exe = os.path.join(d, "pelago")
            with open(exe, "wb") as f:
                f.write(b"x" * 10)
            path = r.deb(exe, "aarch64-linux", "0.1.0", d)
            data = open(path, "rb").read()
            self.assertTrue(data.startswith(b"!<arch>\n"))
            self.assertIn(b"control.tar.gz", data)
            self.assertIn(b"data.tar.gz", data)
            self.assertTrue(path.endswith("pelago-0.1.0-aarch64-linux.deb"))


if __name__ == "__main__":
    unittest.main()
