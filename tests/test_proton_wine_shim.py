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

if __name__ == '__main__':
    unittest.main()
