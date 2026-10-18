#!/usr/bin/env python3
"""Package identity survives a bootstrapper launching from a subdirectory."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SHIM = Path(__file__).resolve().parents[1] / 'scripts/proton-wine-shim.sh'

class PackageManifestTests(unittest.TestCase):
    def test_manifest_is_inherited_and_an_explicit_override_is_preserved(self):
        with tempfile.TemporaryDirectory(prefix='ferestre shim ') as tmp:
            root = Path(tmp)
            proton = root / 'runtime'
            proton.mkdir()
            launcher = proton / 'proton'
            launcher.write_text('#!/bin/sh\nprintf "%s" "$WINE_PACKAGE_MANIFEST"\n')
            launcher.chmod(0o755)
            game = root / 'Game package'
            game.mkdir()
            manifest = game / 'MicrosoftGame.Config'
            manifest.write_text('<Game/>')
            env = {k:v for k,v in os.environ.items() if k not in
                   ['WINE_PACKAGE_MANIFEST', 'FERESTRE_IMAGE_VIEW']}
            env.update(PROTON_DIR=str(proton), STEAM_COMPAT_DATA_PATH=str(root/'prefix'),
                       XODUS_GAMES_DIR=str(root/'games'))
            result = subprocess.run([str(SHIM), 'child\\Game.exe'], env=env, cwd=game,
                                    check=True, capture_output=True, text=True)
            self.assertEqual(result.stdout, 'Z:'+str(manifest).replace('/', chr(92)))
            env['WINE_PACKAGE_MANIFEST'] = 'Z:\\explicit\\appxmanifest.xml'
            result = subprocess.run([str(SHIM), 'child\\Game.exe'], env=env, cwd=game,
                                    check=True, capture_output=True, text=True)
            self.assertEqual(result.stdout, env['WINE_PACKAGE_MANIFEST'])

class GamingServicesWaitTests(unittest.TestCase):
    """Party waits up to ServiceRetryTimeout for Gaming Services, which Wine lacks."""

    def run_shim(self, root, system_reg=None):
        proton = root / 'runtime'
        proton.mkdir()
        calls = root / 'calls'
        launcher = proton / 'proton'
        launcher.write_text('#!/bin/sh\n'
                            'printf "%s|%s|%s\\n" "$*" "${WINE_DLL_FILE_MAP-unset}" '
                            '"${WINE_PACKAGE_MANIFEST-unset}" >> "$CALLS"\n')
        launcher.chmod(0o755)
        prefix = root / 'prefix'
        if system_reg is not None:
            (prefix / 'pfx').mkdir(parents=True)
            (prefix / 'pfx/system.reg').write_text(system_reg)
        env = {k: v for k, v in os.environ.items() if k not in ['FERESTRE_IMAGE_VIEW']}
        env.update(PROTON_DIR=str(proton), STEAM_COMPAT_DATA_PATH=str(prefix),
                   XODUS_GAMES_DIR=str(root/'games'), CALLS=str(calls),
                   WINE_DLL_FILE_MAP='Game.exe=3', WINE_PACKAGE_MANIFEST='Z:\\Game.config')
        subprocess.run([str(SHIM), 'Game.exe'], env=env, cwd=root, check=True,
                       capture_output=True, text=True)
        return calls.read_text().splitlines()

    def test_a_new_prefix_gets_the_timeout_before_the_game_starts(self):
        with tempfile.TemporaryDirectory(prefix='ferestre shim ') as tmp:
            calls = self.run_shim(Path(tmp))
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0], 'run reg add HKLM\\Microsoft\\GamingServices '
                                   '/v ServiceRetryTimeout /t REG_DWORD /d 0 /f|unset|unset')
        # The game itself still gets the decrypted images and its identity.
        self.assertEqual(calls[1], 'run Game.exe|Game.exe=3|Z:\\Game.config')

    def test_a_prefix_that_has_a_value_is_not_touched(self):
        reg = ('WINE REGISTRY Version 2\n\n'
               '[Microsoft\\\\GamingServices] 1791675534\n#time=1dd\n'
               '"ServiceRetryTimeout"=dword:00002710\n')
        with tempfile.TemporaryDirectory(prefix='ferestre shim ') as tmp:
            calls = self.run_shim(Path(tmp), reg)
        self.assertEqual(calls, ['run Game.exe|Game.exe=3|Z:\\Game.config'])

    def test_the_same_value_name_under_another_key_does_not_count(self):
        reg = ('WINE REGISTRY Version 2\n\n'
               '[Microsoft\\\\GamingServicesNot] 1\n"ServiceRetryTimeout"=dword:00000000\n'
               '[Software\\\\Microsoft\\\\GamingServices] 1\n"ServiceRetryTimeout"=dword:00000000\n')
        with tempfile.TemporaryDirectory(prefix='ferestre shim ') as tmp:
            calls = self.run_shim(Path(tmp), reg)
        self.assertEqual(len(calls), 2)
        self.assertTrue(calls[0].startswith('run reg add HKLM\\Microsoft\\GamingServices '))

class PlayStationPadTests(unittest.TestCase):
    """Sony pads go through SDL so a GDK title sees them as XInput gamepads (issue 10)."""
    SONY = '0x054C/0x05C4,0x054C/0x09CC,0x054C/0x0BA0,0x054C/0x0CE6,0x054C/0x0DF2'

    def run_shim(self, root, **extra):
        proton = root / 'runtime'
        proton.mkdir()
        calls = root / 'calls'
        launcher = proton / 'proton'
        launcher.write_text('#!/bin/sh\n'
                            'printf "%s|%s\\n" "$*" "${PROTON_DISABLE_HIDRAW-unset}" >> "$CALLS"\n')
        launcher.chmod(0o755)
        env = {k: v for k, v in os.environ.items()
               if k not in ['FERESTRE_IMAGE_VIEW', 'PROTON_DISABLE_HIDRAW']}
        env.update(PROTON_DIR=str(proton), STEAM_COMPAT_DATA_PATH=str(root/'prefix'),
                   XODUS_GAMES_DIR=str(root/'games'), CALLS=str(calls), **extra)
        subprocess.run([str(SHIM), 'Game.exe'], env=env, cwd=root, check=True,
                       capture_output=True, text=True)
        return calls.read_text().splitlines()

    def test_sony_pads_default_to_sdl_for_the_session_and_the_game(self):
        with tempfile.TemporaryDirectory(prefix='ferestre shim ') as tmp:
            calls = self.run_shim(Path(tmp))
        # The prefix's reg add may start the Wine session the game then joins.
        self.assertEqual([c.split('|')[1] for c in calls], [self.SONY, self.SONY])

    def test_an_explicit_value_is_kept_even_empty_or_zero(self):
        for value in ['0', '', '0x054C/0x09CC']:
            with tempfile.TemporaryDirectory(prefix='ferestre shim ') as tmp:
                calls = self.run_shim(Path(tmp), PROTON_DISABLE_HIDRAW=value)
            self.assertEqual(calls[-1], 'run Game.exe|' + value, value)

if __name__ == '__main__':
    unittest.main()
