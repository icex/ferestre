#!/usr/bin/env python3
"""Release signing: what packaging/sign-release.sh signs, and what it refuses.

Every key here is a throwaway pair generated inside the test. The release key
lives only in a CI secret and is never generated or read by a test.

Needs minisign. Without it the tests skip, unless FERESTRE_REQUIRE_MINISIGN=1
(CI sets it), in which case a missing minisign is a failure.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / 'packaging/sign-release.sh'


def need_minisign(test):
    if shutil.which('minisign'):
        return
    if os.environ.get('FERESTRE_REQUIRE_MINISIGN') == '1':
        test.fail('minisign is not installed and FERESTRE_REQUIRE_MINISIGN=1')
    test.skipTest('minisign is not installed')


def keypair(root, password):
    """A test key pair: (secret key file text, public key base64 line)."""
    pub, sec = root / 'test.pub', root / 'test.key'
    args = ['minisign', '-G', '-f', '-p', str(pub), '-s', str(sec)]
    if password is None:
        args.append('-W')
        stdin = ''
    else:
        stdin = f'{password}\n{password}\n'
    subprocess.run(args, input=stdin, text=True, check=True, capture_output=True)
    text = sec.read_text()
    sec.unlink()
    return text, pub.read_text().splitlines()[1], pub


class SignReleaseTests(unittest.TestCase):
    def setUp(self):
        need_minisign(self)
        self.tmp = tempfile.TemporaryDirectory(prefix='ferestre sign ')
        self.root = Path(self.tmp.name)
        self.out = self.root / 'out'
        self.out.mkdir()
        (self.out / 'ferestre-1.2.3-x86_64.AppImage').write_bytes(b'appimage bytes')
        (self.out / 'ferestre-1.2.3-x86_64.tar.zst').write_bytes(b'tarball bytes')
        self.sums()

    def tearDown(self):
        self.tmp.cleanup()

    def sums(self):
        names = sorted(p.name for p in self.out.iterdir()
                       if p.name != 'SHA256SUMS' and not p.name.endswith('.minisig'))
        result = subprocess.run(['sha256sum', *(f'./{n}' for n in names)], cwd=self.out,
                                check=True, capture_output=True, text=True)
        (self.out / 'SHA256SUMS').write_text(result.stdout)

    def sign(self, secret, password=None, version='1.2.3', **extra):
        env = {k: v for k, v in os.environ.items()
               if k not in ('MINISIGN_SECRET_KEY', 'MINISIGN_PASSWORD',
                            'FERESTRE_EXPECT_PUBKEY_IN')}
        env.update(MINISIGN_SECRET_KEY=secret, TMPDIR=str(self.root), **extra)
        if password is not None:
            env['MINISIGN_PASSWORD'] = password
        return subprocess.run([str(SCRIPT), str(self.out), version], env=env,
                              capture_output=True, text=True)

    def verify(self, pub, name):
        return subprocess.run(['minisign', '-V', '-p', str(pub), '-m', str(self.out / name)],
                              capture_output=True, text=True)

    def test_every_artefact_and_the_sums_are_signed_with_the_file_name_trusted(self):
        secret, _, pub = keypair(self.root, 'test password')
        result = self.sign(secret, 'test password')
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ('ferestre-1.2.3-x86_64.AppImage', 'ferestre-1.2.3-x86_64.tar.zst',
                     'SHA256SUMS'):
            check = self.verify(pub, name)
            self.assertEqual(check.returncode, 0, check.stderr)
            self.assertIn(f'Trusted comment: file:{name} version:1.2.3\n', check.stdout)
        # Nothing but the artefacts and their signatures is left for upload.
        self.assertEqual(sorted(p.name for p in self.out.iterdir()), sorted([
            'SHA256SUMS', 'SHA256SUMS.minisig',
            'ferestre-1.2.3-x86_64.AppImage', 'ferestre-1.2.3-x86_64.AppImage.minisig',
            'ferestre-1.2.3-x86_64.tar.zst', 'ferestre-1.2.3-x86_64.tar.zst.minisig']))
        self.assertEqual([p for p in self.root.iterdir() if p.name.startswith('ferestre-sign')], [])

    def test_a_key_without_a_password_signs_too(self):
        secret, _, pub = keypair(self.root, None)
        result = self.sign(secret)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.verify(pub, 'SHA256SUMS').returncode, 0)

    def test_a_signature_does_not_cover_a_changed_artefact(self):
        secret, _, pub = keypair(self.root, None)
        self.assertEqual(self.sign(secret).returncode, 0)
        (self.out / 'ferestre-1.2.3-x86_64.tar.zst').write_bytes(b'tampered')
        self.assertNotEqual(self.verify(pub, 'ferestre-1.2.3-x86_64.tar.zst').returncode, 0)

    def test_an_artefact_that_no_longer_matches_its_sum_is_not_signed(self):
        secret, _, _ = keypair(self.root, None)
        (self.out / 'ferestre-1.2.3-x86_64.AppImage').write_bytes(b'changed in transit')
        result = self.sign(secret)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('do not match their SHA256SUMS', result.stderr)
        self.assertEqual(list(self.out.glob('*.minisig')), [])

    def test_an_artefact_missing_from_the_sums_is_not_signed(self):
        secret, _, _ = keypair(self.root, None)
        (self.out / 'extra.bin').write_bytes(b'never checked')
        result = self.sign(secret)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('extra.bin is not listed in SHA256SUMS', result.stderr)
        self.assertEqual(list(self.out.glob('*.minisig')), [])

    def test_a_wrong_password_signs_nothing(self):
        secret, _, _ = keypair(self.root, 'right')
        result = self.sign(secret, 'wrong')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(self.out.glob('*.minisig')), [])

    def test_a_launcher_that_does_not_trust_the_signing_key_stops_the_release(self):
        secret, pubkey, _ = keypair(self.root, None)
        launcher = self.root / 'ferestre'
        launcher.write_bytes(b'\x7fELF... RWQPLACEHOLDERPLACEHOLDER ...')
        result = self.sign(secret, FERESTRE_EXPECT_PUBKEY_IN=str(launcher))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('does not contain the public key', result.stderr)
        self.assertEqual(list(self.out.glob('*.minisig')), [])

        launcher.write_bytes(b'\x7fELF... ' + pubkey.encode() + b' ...')
        result = self.sign(secret, FERESTRE_EXPECT_PUBKEY_IN=str(launcher))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_tag_is_not_taken_for_the_version(self):
        secret, _, _ = keypair(self.root, None)
        for tag in ('v1.2.3', 'runtime-11.0.20260907', '', '1.2.3+build', '.1.2', '-1'):
            result = self.sign(secret, version=tag)
            self.assertNotEqual(result.returncode, 0, tag)
            self.assertIn('not a usable release version', result.stderr)
        self.assertEqual(list(self.out.glob('*.minisig')), [])

    def test_signing_needs_a_key(self):
        result = self.sign('')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('MINISIGN_SECRET_KEY is empty', result.stderr)


if __name__ == '__main__':
    unittest.main()
