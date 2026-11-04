#!/usr/bin/env python3
"""scripts/install-xodus-proton.sh against a stand-in build tree.

The Makefile here installs a tiny compatibility tool the way Proton's does
(into ~/.steam/root/compatibilitytools.d/<build name>, announcing it), and the
container tools are stubs that record whether newgrp was used.
"""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/install-xodus-proton.sh"

MAKEFILE = """\
BUILD_NAME := xodus
SRCDIR := {source}
CONTAINER_ENGINE := {engine}
DEST := $(HOME)/.steam/root/compatibilitytools.d/$(BUILD_NAME)
install:
\tmkdir -p $(DEST)/files/lib/wine/x86_64-windows $(DEST)/files/lib/wine/vkd3d-proton/x86_64-windows
\tprintf 'subprocess.call(close_fds=False)\\n' > $(DEST)/proton
\tprintf 'unrecognised index' > $(DEST)/files/lib/wine/x86_64-windows/xgameruntime.dll
\tprintf 'retain_recording_allocators' > $(DEST)/files/lib/wine/vkd3d-proton/x86_64-windows/d3d12core.dll
\t@echo "Installed Proton to "$(DEST)
"""


class InstallScriptTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="ferestre-install-runtime-test-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.home = self.root / "home"
        (self.home / ".steam/root").mkdir(parents=True)
        self.build = self.root / "build"
        self.build.mkdir()
        self.repo = self.root / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        self.source = self.root / "source"
        (self.source / "wine/dlls/xgameruntime").mkdir(parents=True)
        (self.source / "vkd3d-proton").mkdir()
        (self.repo / "scripts/source-pins.py").write_text("import os,sys\nfrom pathlib import Path\nif sys.argv[1]=='verify':\n with Path(os.environ['CALLS']).open('a') as f:f.write('verify '+sys.argv[2]+'\\n')\n if os.environ.get('BAD_SOURCE')=='1':sys.exit(1)\nelse: print('fixture.c')\n")
        self.capabilities = self.root / "capabilities.record"
        self.stub(self.repo / "scripts/write-capabilities.sh", 'echo "$1" >> "$CAPABILITIES"')
        self.calls = self.root / "calls"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        # newgrp feeds its standard input to a shell in the new group.
        self.stub(self.bin / "newgrp", 'echo "newgrp $*" >> "$CALLS"\nexec sh')
        self.packaged = self.root / "packaged"
        self.packaged.mkdir()
        self.packaged.chmod(0o555)
        self.addCleanup(self.packaged.chmod, 0o755)

    def stub(self, path, body):
        path.write_text(f"#!/bin/sh\n{body}\n")
        path.chmod(0o755)

    def run_script(self, engine, docker_works=False, group=None, session_groups="user wheel", bad_source=False):
        (self.build / "Makefile").write_text(MAKEFILE.format(engine=engine,source=self.source))
        self.stub(self.bin / "docker", "exit 0" if docker_works else "exit 1")
        self.stub(self.bin / "getent", f"echo '{group}'" if group else "exit 2")
        self.stub(self.bin / "id", f'[ "$1" = -nG ] && echo "{session_groups}"')
        env = {k: v for k, v in os.environ.items() if k != "TOOL_DIR"}
        env.update(HOME=str(self.home), PATH=f"{self.bin}:{os.environ['PATH']}",
                   BUILD_DIR=str(self.build), REPO_DIR=str(self.repo),
                   WINE_SRC=str(self.source / "wine"), VKD3D_SRC=str(self.source / "vkd3d-proton"),
                   BAD_SOURCE="1" if bad_source else "0",
                   XODUS_PROTON_DIR=str(self.packaged), CALLS=str(self.calls),
                   CAPABILITIES=str(self.capabilities), J="1")
        p = subprocess.run([str(SCRIPT)], env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode != 0, bad_source, p.stdout + p.stderr)
        return p

    def installed(self):
        return str(self.home / ".steam/root/compatibilitytools.d/xodus")

    def newgrp_used(self):
        return self.calls.exists() and "newgrp" in self.calls.read_text()

    def test_a_source_pin_failure_stops_before_installing_anything(self):
        self.run_script("podman", bad_source=True)
        self.assertFalse(Path(self.installed()).exists())
        self.assertFalse(self.capabilities.exists())

    def test_podman_builds_without_a_docker_group(self):
        self.run_script("podman")
        self.assertFalse(self.newgrp_used())

    def test_the_tree_make_installed_is_checked_not_xodus_proton_dir(self):
        self.run_script("podman")
        self.assertEqual(self.capabilities.read_text().split(), [self.installed()])

    def test_docker_reachable_already_needs_no_newgrp(self):
        self.run_script("docker", docker_works=True, group="docker:x:970:user")
        self.assertFalse(self.newgrp_used())

    def test_newgrp_is_the_fallback_for_a_session_without_the_group(self):
        self.run_script("docker", group="docker:x:970:user")
        self.assertTrue(self.newgrp_used())
        self.assertEqual(self.capabilities.read_text().split(), [self.installed()])

    def test_a_session_already_in_the_group_runs_make_directly(self):
        self.run_script("docker", group="docker:x:970:user", session_groups="user docker")
        self.assertFalse(self.newgrp_used())


if __name__ == "__main__":
    unittest.main()
