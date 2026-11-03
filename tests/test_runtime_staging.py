#!/usr/bin/env python3
"""Runtime test staging must never share writable inodes with production."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
SCRIPT=Path(__file__).resolve().with_name('stage-runtime.sh')
class Staging(unittest.TestCase):
    def stage(self,other=None):
        with tempfile.TemporaryDirectory() as source_root,tempfile.TemporaryDirectory(dir=other) as target_root:
            source=Path(source_root)/'runtime';source.mkdir()
            file=source/'runtime.dll';file.write_bytes(b'original runtime');file.chmod(0o755)
            (source/'alias').symlink_to('runtime.dll')
            target=Path(target_root)/'staged'
            subprocess.run([SCRIPT,source,target],check=True)
            self.assertTrue((target/'alias').is_symlink())
            self.assertEqual((target/'runtime.dll').read_bytes(),file.read_bytes())
            self.assertNotEqual((file.stat().st_dev,file.stat().st_ino),((target/'runtime.dll').stat().st_dev,(target/'runtime.dll').stat().st_ino))
            (target/'runtime.dll').write_bytes(b'test replacement')
            self.assertEqual(file.read_bytes(),b'original runtime')
            result=subprocess.run([SCRIPT,source,target],capture_output=True)
            self.assertNotEqual(result.returncode,0)
    def test_staged_dlls_can_change_without_modifying_the_runtime(self):self.stage()
    def test_copy_also_works_on_a_separate_filesystem(self):
        if not os.access('/dev/shm',os.W_OK):self.skipTest('/dev/shm unavailable')
        self.stage('/dev/shm')
if __name__=='__main__':unittest.main()
