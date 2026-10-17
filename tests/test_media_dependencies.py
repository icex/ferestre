#!/usr/bin/env python3
"""Exercise doctor with a real ELF dependency missing, then available to Proton."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class MediaDependencies(unittest.TestCase):
    def test_missing_library_is_actionable_and_runtime_libraries_are_searched(self):
        binary = Path(os.environ.get("FERESTRE_BINARY", "target/debug/ferestre")).resolve()
        with tempfile.TemporaryDirectory(prefix="ferestre-media-") as temporary:
            home = Path(temporary)
            runtime = home / "runtime"
            lib = runtime / "files/lib/x86_64-linux-gnu"
            plugins = lib / "gstreamer-1.0"
            plugins.mkdir(parents=True)
            (runtime / "proton").write_text("#!/bin/sh\nexit 0\n")
            (runtime / "version").write_text("0 fixture-runtime\n")
            (home / "dependency.c").write_text("int ferestre_media_fixture(void) { return 1; }\n")
            (home / "plugin.c").write_text("extern int ferestre_media_fixture(void); int plugin(void) { return ferestre_media_fixture(); }\n")
            dependency = home / "libferestre_media_fixture.so.1"
            subprocess.run(["cc", "-shared", "-fPIC", "-Wl,-soname,libferestre_media_fixture.so.1",
                            "-o", dependency, home / "dependency.c"], check=True)
            subprocess.run(["cc", "-shared", "-fPIC", "-o", plugins / "libgstlibav.so",
                            home / "plugin.c", dependency], check=True)
            env = dict(os.environ, HOME=temporary, XODUS_PROTON_DIR=str(runtime),
                       XODUS_GAMES_DIR=str(home / "games"), XODUS_CLI_DIR=str(home / "client"),
                       XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
                       XDG_CACHE_HOME=str(home / "cache"))
            env.pop("LD_LIBRARY_PATH", None)

            def media_check():
                result = subprocess.run([binary, "doctor", "--json"], env=env, capture_output=True, text=True)
                checks = json.loads(result.stdout)["checks"]
                return next(check for check in checks if check["section"] == "media")

            missing = media_check()
            self.assertEqual(missing["status"], "warn")
            self.assertIn(dependency.name, missing["detail"])
            self.assertIn("compat-ffmpeg4", missing["hint"])
            dependency.rename(lib / dependency.name)
            resolved = media_check()
            self.assertEqual(resolved["status"], "ok")
            self.assertIn("dependencies available", resolved["name"])


if __name__ == "__main__":
    unittest.main()
