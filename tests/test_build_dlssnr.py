#!/usr/bin/env python3
"""scripts/build-dlssnr.sh's own logic, without a container or the network.

The script runs `main` only when executed, so these tests source it and call
its functions on small local tarballs.
"""
import hashlib
import io
from pathlib import Path
import os
import subprocess
import tarfile
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/build-dlssnr.sh'


def call(*args):
    return subprocess.run(['bash', '-c', 'source "$1"; shift; "$@"', 'test', str(SCRIPT), *args],
                          capture_output=True, text=True)


def make_tarball(path, files, mtime):
    with tarfile.open(path, 'w:gz') as tar:
        for name, data in files.items():
            info = tarfile.TarInfo('./' + name)
            info.size = len(data)
            info.mtime = mtime
            info.uid = 1234
            info.uname = 'builder'
            tar.addfile(info, io.BytesIO(data))


class BuildDlssnrTests(unittest.TestCase):
    def test_a_tarball_with_a_model_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            for model in ('CC_SILVER_AARDWOLD.bin', 'CC_SMALL_FP8.bin', 'dlssnr.bin', 'dlssnr-small.bin'):
                with self.subTest(model=model):
                    path = Path(tmp) / 'pkg.tar.gz'
                    make_tarball(path, {'install.sh': b'#!/bin/sh\n', 'dlssnr-amd/' + model: b'x'}, 1)
                    result = call('refuse_models', str(path))
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('no NVIDIA model', result.stderr)

    def test_hash_files_beside_a_model_name_are_not_models(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'pkg.tar.gz'
            make_tarball(path, {'install.sh': b'', 'optimized/CC_SILVER_AARDWOLD_OPTIMIZED.bin.sha256': b'',
                                'model-tools/CC_SILVER_AARDWOLD.bin.sha256': b''}, 1)
            result = call('refuse_models', str(path))
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_equal_contents_repack_to_an_equal_tarball_without_a_top_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            files = {'install.sh': b'#!/bin/sh\n', 'dlssnr-amd/a.txt': b'a', 'reshade/b.dll': b'b'}
            make_tarball(tmp / 'one.tar.gz', files, 1000)
            make_tarball(tmp / 'two.tar.gz', dict(reversed(list(files.items()))), 2000)
            for name in ('one', 'two'):
                result = call('repack', str(tmp / f'{name}.tar.gz'), str(tmp / f'{name}.out.tar.gz'), '1794517079')
                self.assertEqual(result.returncode, 0, result.stderr)
                time.sleep(1.1)  # a gzip timestamp would differ
            one = (tmp / 'one.out.tar.gz').read_bytes()
            self.assertEqual(hashlib.sha256(one).digest(),
                             hashlib.sha256((tmp / 'two.out.tar.gz').read_bytes()).digest())
            with tarfile.open(tmp / 'one.out.tar.gz') as tar:
                members = tar.getmembers()
            names = [m.name for m in members]
            self.assertEqual(names, sorted(names))
            self.assertIn('./install.sh', names)
            for member in members:
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 1794517079), member.name)

    def test_a_non_empty_output_dir_is_refused_before_anything_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / 'stale').write_text('x')
            env = dict(os.environ, FERESTRE_CONTAINER='/bin/false')
            result = subprocess.run(['bash', str(SCRIPT), tmp], capture_output=True, text=True, env=env)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('not empty', result.stderr)
            self.assertEqual(sorted(p.name for p in Path(tmp).iterdir()), ['stale'])

    def run_with_fake_engine(self, tmp, files, commit=None):
        """main() against an engine whose `run` only reads the script and whose
        `cp` hands back prepared upstream tarballs."""
        tmp = Path(tmp)
        pins = subprocess.check_output(['python3', str(ROOT / 'scripts/source-pins.py'), 'package',
                                        'dlssnr-amd', 'commit'], text=True).strip()
        version = subprocess.check_output(['python3', str(ROOT / 'scripts/source-pins.py'), 'package',
                                           'dlssnr-amd', 'version'], text=True).strip()
        result = tmp / 'result'
        result.mkdir()
        for arch in ('x86_64', 'i686'):
            make_tarball(result / f'DLSSNR-AMD-Vulkan-Linux-{version}-{arch}.tar.gz', files, 5)
        (result / 'commit').write_text((commit or pins) + '\n')
        (result / 'mtime').write_text('1794517079\n')
        engine = tmp / 'engine'
        engine.write_text('#!/bin/bash\n'
                          'echo "$*" >> "$FAKE_LOG"\n'
                          'case $1 in\n'
                          '  run) cat > "$FAKE_SCRIPT" ;;\n'
                          '  cp) cp -r "$FAKE_RESULT" "$3" ;;\n'
                          'esac\n')
        engine.chmod(0o755)
        env = dict(os.environ, FERESTRE_CONTAINER=str(engine), FAKE_RESULT=str(result),
                   FAKE_LOG=str(tmp / 'log'), FAKE_SCRIPT=str(tmp / 'script'), TMPDIR=str(tmp))
        out = tmp / 'out'
        run = subprocess.run(['bash', str(SCRIPT), str(out)], capture_output=True, text=True, env=env)
        return run, out, version, tmp

    def test_the_output_is_exactly_two_tarballs_the_sources_and_sums_of_all_three(self):
        with tempfile.TemporaryDirectory() as tmp:
            run, out, version, tmp = self.run_with_fake_engine(tmp, {'install.sh': b'#!/bin/sh\n'})
            self.assertEqual(run.returncode, 0, run.stderr)
            names = [f'ferestre-dlssnr-{version}-i686.tar.gz', f'ferestre-dlssnr-{version}-x86_64.tar.gz',
                     'SOURCES.txt']
            self.assertEqual(sorted(p.name for p in out.iterdir()), sorted(names + ['SHA256SUMS']))
            listed = [line.split()[1] for line in (out / 'SHA256SUMS').read_text().splitlines()]
            self.assertEqual(sorted(listed), sorted(names))
            subprocess.run(['sha256sum', '--strict', '-c', 'SHA256SUMS'], cwd=out, check=True,
                           capture_output=True)
            sources = (out / 'SOURCES.txt').read_text()
            self.assertIn('9503f6b180df45305b38fbd21cd9f705164f2ab1', sources)
            self.assertIn('OptiScaler (GPL-3.0)', sources)
            # The pinned image ran the script it was handed, and nothing was bind-mounted.
            log = (tmp / 'log').read_text()
            self.assertRegex(log, r'run -i --name \S+ .*docker\.io/library/ubuntu:24\.04@sha256:[0-9a-f]{64} ')
            self.assertNotRegex(log, r'(^|\s)(-v|--volume|--mount)(\s|=)')
            self.assertEqual((tmp / 'script').read_text(), SCRIPT.read_text())
            # The temporary work dir is gone; only what the test made is left.
            self.assertEqual(sorted(p.name for p in tmp.iterdir()),
                             ['engine', 'log', 'out', 'result', 'script'])

    def test_a_model_in_upstreams_tarball_leaves_the_output_empty(self):
        with tempfile.TemporaryDirectory() as tmp:
            run, out, _, _ = self.run_with_fake_engine(
                tmp, {'install.sh': b'', 'dlssnr-amd/CC_SMALL_FP8.bin': b'x'})
            self.assertNotEqual(run.returncode, 0)
            self.assertIn('no NVIDIA model', run.stderr)
            self.assertEqual(list(out.iterdir()), [])

    def test_a_build_of_another_commit_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            run, out, _, _ = self.run_with_fake_engine(tmp, {'install.sh': b''}, commit='a' * 40)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn('pinned commit', run.stderr)
            self.assertEqual(list(out.iterdir()), [])

    def test_the_build_runs_upstreams_release_steps_at_the_pin(self):
        text = SCRIPT.read_text()
        for needle in ('"$pins" package dlssnr-amd repository', 'package dlssnr-amd commit',
                       'package dlssnr-amd version', 'bash fetch_deps.sh',
                       'bash linux/build/build_vulkan_loader.sh',
                       'NR_VERSION=$DLSSNR_VERSION NR_ARCH=$arch NR_SIGN=1 bash linux/build/build_package.sh',
                       'ARCHES=(x86_64 i686)', '${FERESTRE_CONTAINER:-docker}'):
            self.assertIn(needle, text)
        self.assertRegex(text, r'\nIMAGE=docker\.io/library/ubuntu:24\.04@sha256:[0-9a-f]{64}\n')


if __name__ == '__main__':
    unittest.main()
