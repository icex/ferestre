#!/usr/bin/env python3
"""Exercise actual CLI control forwarding and allowlisted error persistence."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

STUB = r'''#!/usr/bin/env python3
import json,os,signal,sys
from pathlib import Path
h=Path(os.environ['HOME']);dest=Path(sys.argv[3]);dest.mkdir(parents=True,exist_ok=True)
if (h/'fail').exists():
 print('raw synthetic secret=SHOULD_NOT_BE_LOGGED',file=sys.stderr)
 print('FERESTRE_TRANSFER_DIAGNOSTIC_V1 transfer-status 403',file=sys.stderr)
 print('FERESTRE_TRANSFER_DIAGNOSTIC_V1 transfer-io 0',file=sys.stderr)
 sys.exit(19)
def stop(*_):
 (dest/'cleaned').touch();sys.exit(0)
signal.signal(signal.SIGTERM,stop)
(dest/'partial').write_bytes(b'verified completed data');(h/'ready').touch()
for line in sys.stdin:
 record=json.loads(line)
 with (h/'controls').open('a') as out:out.write(record['command']+'\n')
'''


class Controls(unittest.TestCase):
    def launch(self, home, managed=True):
        client=home/'client';client.mkdir()
        backend=client/'xodus-cli';backend.write_text(STUB);backend.chmod(0o755)
        binary=Path(os.environ.get('FERESTRE_TEST_CLI','target/debug/ferestre')).resolve()
        env=dict(os.environ,HOME=str(home),XDG_CONFIG_HOME=str(home/'config'),XDG_STATE_HOME=str(home/'state'),XDG_CACHE_HOME=str(home/'cache'),XDG_DATA_HOME=str(home/'data'),FERESTRE_TITLES=str(home/'recipes'),XODUS_CLI_DIR=str(client),XODUS_GAMES_DIR=str(home/'games'),FERESTRE_GUI_CANCELLATION='1')
        if not managed:env.pop("FERESTRE_GUI_CANCELLATION",None)
        return subprocess.Popen([binary,'install','9ZZCONTROL01'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)

    def wait_for(self,predicate,process):
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            if predicate():return
            if process.poll() is not None:self.fail(process.stderr.read().decode())
            time.sleep(.02)
        self.fail('control did not reach the private downloader')

    def test_pause_resume_and_stop_cross_the_real_cli_boundary(self):
        with tempfile.TemporaryDirectory() as tmp:
            home=Path(tmp);process=self.launch(home)
            try:
                self.wait_for(lambda:(home/'ready').exists(),process)
                for byte,command in [(b'p','pause'),(b'r','resume')]:
                    process.stdin.write(byte);process.stdin.flush()
                    self.wait_for(lambda: (home/'controls').exists() and command in (home/'controls').read_text().splitlines(),process)
                    self.assertIsNone(process.poll())
                process.stdin.write(b'x');process.stdin.flush();process.stdin.close();process.stdin=None
                _,stderr=process.communicate(timeout=10)
                self.assertEqual(process.returncode,130,stderr.decode())
                self.assertEqual((home/'controls').read_text().splitlines(),['pause','resume'])
                self.assertTrue((home/'games/9zzcontrol01/cleaned').exists())
                self.assertEqual((home/'games/9zzcontrol01/partial').read_bytes(),b'verified completed data')
            finally:
                if process.poll() is None:process.kill();process.wait()

    def test_http_reason_survives_generic_exit_and_raw_output_is_not_saved(self):
        with tempfile.TemporaryDirectory() as tmp:
            home=Path(tmp);(home/'fail').touch();process=self.launch(home,managed=False)
            stdout,stderr=process.communicate(timeout=10)
            result=subprocess.CompletedProcess(process.args,process.returncode,stdout.decode(),stderr.decode())
            self.assertNotEqual(result.returncode,0)
            self.assertIn('HTTP 403',result.stderr)
            self.assertIn('fresh link',result.stderr)
            entries=json.loads((home/'state/ferestre/transfer-diagnostics.json').read_text())
            self.assertTrue(any(e['diagnostic']=={'stage':'transfer-status','http_status':403} for e in entries))
            self.assertNotIn('SHOULD_NOT_BE_LOGGED',json.dumps(entries))


if __name__=='__main__':unittest.main()
