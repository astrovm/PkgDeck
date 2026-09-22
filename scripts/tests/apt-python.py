#!/usr/bin/env python3
"""Synthetic contracts for the embedded host APT query; never opens host APT."""
import builtins
import contextlib
import io
from pathlib import Path
import runpy
import sys
import types
import unittest
from unittest.mock import patch

HELPER = Path(__file__).resolve().parents[2] / "crates/pkgdeck-core/src/apt-query.py"
query = runpy.run_path(str(HELPER))["query"]


class AptFixture:
    def __init__(self, phasing_api=True):
        self.packages = []
        self.records = {}
        self.config = {}
        self.apt_pkg = types.ModuleType("apt_pkg")
        self.apt_pkg.init = lambda: None
        self.apt_pkg.config = types.SimpleNamespace(set=self.config.__setitem__)
        self.apt_pkg.Cache = lambda progress: types.SimpleNamespace(packages=self.packages)
        policy = types.SimpleNamespace(get_candidate_ver=lambda package: package.candidate)
        if phasing_api:
            policy.phasing_applied = lambda package: package.phased
        self.apt_pkg.DepCache = lambda cache: policy
        self.apt_pkg.SELSTATE_HOLD = 2
        self.apt_pkg.version_compare = lambda left, right: int(left) - int(right)
        fixture = self

        class Records:
            def lookup(self, reference):
                self.data = fixture.records[reference[1]]
                self.short_desc = self.data["summary"]
                self.long_desc = self.data["description"]
                self.homepage = self.data["homepage"]

            def __contains__(self, key):
                return key in self.data

            def __getitem__(self, key):
                return self.data[key]

        self.apt_pkg.PackageRecords = lambda cache: Records()
        apt = types.ModuleType("apt")
        apt.progress = types.ModuleType("apt.progress")
        apt.progress.base = types.ModuleType("apt.progress.base")
        apt.progress.base.OpProgress = lambda: object()
        self.modules = {"apt_pkg": self.apt_pkg, "apt": apt,
                        "apt.progress": apt.progress, "apt.progress.base": apt.progress.base}

    def add(self, name="synthetic", arch="amd64", installed="1", candidate="2",
            held=False, phased=False, summary="Synthetic café", fields=None, files=True):
        index = len(self.packages)
        file_list = [(types.SimpleNamespace(id=0), index)] if files else []
        def version(value):
            return types.SimpleNamespace(ver_str=value, arch=arch, file_list=file_list) if value else None
        self.packages.append(types.SimpleNamespace(name=name, current_ver=version(installed),
                             candidate=version(candidate), selected_state=2 if held else 0,
                             phased=phased))
        self.records[index] = {"summary": summary, "description": "Synthetic description",
                               "homepage": "https://example.invalid", "Depends": "dependency (>= 1)"}
        self.records[index].update(fields or {})

    def query(self, mode="installed", needle="", arch="amd64"):
        with patch.dict(sys.modules, self.modules):
            return query(mode, needle, arch)


class AptQueryTests(unittest.TestCase):
    def test_updates_exclude_held_and_phased(self):
        fixture = AptFixture()
        fixture.add("updatable")
        fixture.add("held", held=True)
        fixture.add("phased", phased=True)
        fixture.add("same", candidate="1")
        fixture.add("downgrade", candidate="0")
        rows = fixture.query()
        self.assertEqual([row["package"]["id"]["name"] for row in rows
                          if row["package"]["update"] == "available"], ["updatable"])
        self.assertEqual(fixture.config, {"Dir::Cache::pkgcache": "", "Dir::Cache::srcpkgcache": ""})

    def test_older_apt_phasing_fallback(self):
        fixture = AptFixture(phasing_api=False)
        fixture.add("phased", fields={"Phased-Update-Percentage": "30"})
        fixture.add("fully-released", fields={"Phased-Update-Percentage": "100"})
        rows = fixture.query()
        self.assertEqual([row["package"]["update"] for row in rows], ["current", "available"])

    def test_details_exact_name_architecture_and_metadata(self):
        fixture = AptFixture()
        fixture.add("synthetic", arch="amd64")
        fixture.add("synthetic", arch="arm64")
        fixture.add("synthetic-extra", arch="arm64")
        rows = fixture.query("details", "synthetic", "arm64")
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["package"]["id"], {"backend": "apt", "name": "synthetic",
                         "architecture": "arm64", "scope": "system"})
        self.assertEqual(rows[0]["description"], "Synthetic description")
        self.assertEqual(rows[0]["dependencies"], ["dependency (>= 1)"])
        self.assertEqual(rows[0]["homepage"], "https://example.invalid")
        self.assertEqual(fixture.query("details", "synthetic", "i386"), [])

    def test_search_unicode_and_missing_versions(self):
        fixture = AptFixture()
        fixture.add("cafe", installed=None, summary="CAFÉ player")
        fixture.add("installed-only", candidate=None, summary="Installed only")
        fixture.add("no-version", installed=None, candidate=None)
        fixture.add("no-record", files=False)
        rows = fixture.query("search", "café")
        self.assertEqual([row["package"]["id"]["name"] for row in rows], ["cafe"])
        self.assertEqual(rows[0]["package"]["update"], "unknown")
        rows = fixture.query()
        self.assertEqual(len(rows), 1)
        self.assertIsNone(rows[0]["package"]["candidate_version"])
        self.assertEqual(rows[0]["package"]["installed_version"], "1")
        self.assertEqual(rows[0]["package"]["update"], "current")

    def test_detect_and_invalid_mode(self):
        fixture = AptFixture()
        self.assertEqual(fixture.query("detect"), [])
        with self.assertRaisesRegex(ValueError, "unknown APT query"):
            fixture.query("erase")

    def test_missing_python_apt_is_actionable(self):
        original_import = builtins.__import__
        def importing(name, *args, **kwargs):
            if name == "apt_pkg":
                raise ImportError("synthetic missing dependency")
            return original_import(name, *args, **kwargs)
        with patch("builtins.__import__", side_effect=importing):
            with self.assertRaisesRegex(RuntimeError, "Install python3-apt on the host"):
                query("detect", "", "amd64")

    def test_cli_invalid_arguments_fail_without_json_success(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(sys, "argv", [str(HELPER)]), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as failure:
                runpy.run_path(str(HELPER), run_name="__main__")
        self.assertEqual(failure.exception.code, 1)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("expected mode, query, architecture", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
