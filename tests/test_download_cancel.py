#!/usr/bin/env python3
"""Exercise the shipped CLI's cancellation protocol with an offline downloader."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


class DownloadCancellation(unittest.TestCase):
    def test_stop_and_window_disappearance_wait_for_cleanup_and_keep_files(self):
        binary = Path(os.environ.get("FERESTRE_TEST_CLI", "target/debug/ferestre")).resolve()
        for existing in (False, True):
            for disappear in (False, True):
                with self.subTest(update=existing, window_disappears=disappear), tempfile.TemporaryDirectory() as tmp:
                    home = Path(tmp)
                    client = home / "client"
                    client.mkdir()
                    stub = client / "xodus-cli"
                    stub.write_text('''#!/usr/bin/env python3
import os, signal, sys, time
from pathlib import Path
dest = Path(sys.argv[3])
def stop(*_):
    time.sleep(0.3)
    (dest / "cleaned").write_text("done")
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
(dest / "partial").write_text("downloaded bytes")
(dest / "ready").write_text(str(os.getpid()))
while True:
    time.sleep(0.05)
''')
                    stub.chmod(0o755)
                    dest = home / "game"
                    record = home / "state/ferestre/installed/9ZZCANCEL001.json"
                    if existing:
                        dest.mkdir()
                        (dest / "original-save").write_text("keep me")
                        record.parent.mkdir(parents=True)
                        record.write_text('{"previous_record":true}')
                    env = dict(os.environ, HOME=tmp, XDG_STATE_HOME=str(home / "state"),
                               XDG_CONFIG_HOME=str(home / "config"), XDG_CACHE_HOME=str(home / "cache"),
                               XODUS_CLI_DIR=str(client), XODUS_GAMES_DIR=str(home / "games"),
                               FERESTRE_TITLES=str(home / "titles"), FERESTRE_GUI_CANCELLATION="1")
                    process = subprocess.Popen([str(binary), "install", "9ZZCANCEL001", str(dest)],
                                               env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                               stderr=subprocess.PIPE)
                    try:
                        deadline = time.monotonic() + 10
                        while not (dest / "ready").exists() and time.monotonic() < deadline:
                            if process.poll() is not None:
                                self.fail(process.stderr.read().decode())
                            time.sleep(0.02)
                        self.assertTrue((dest / "ready").exists())
                        downloader = int((dest / "ready").read_text())
                        if not disappear:
                            process.stdin.write(b"x")
                            process.stdin.flush()
                        process.stdin.close()
                        process.stdin = None
                        _, stderr = process.communicate(timeout=10)
                        self.assertEqual(process.returncode, 130, stderr.decode())
                        self.assertTrue((dest / "cleaned").exists(), "launcher exited before downloader cleanup")
                        self.assertTrue((dest / "partial").exists())
                        with self.assertRaises(ProcessLookupError):
                            os.kill(downloader, 0)
                        if existing:
                            self.assertEqual((dest / "original-save").read_text(), "keep me")
                            self.assertEqual(record.read_text(), '{"previous_record":true}')
                        else:
                            self.assertFalse(record.exists(), "stopped download was recorded as installed")
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait()


if __name__ == "__main__":
    unittest.main()
