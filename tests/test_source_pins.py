#!/usr/bin/env python3
"""Source provenance and refusal checks; all fixtures are local Git repositories."""
import copy
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('source_pins', ROOT/'scripts/source-pins.py')
pins_module=importlib.util.module_from_spec(spec);spec.loader.exec_module(pins_module)

class SourcePinsTests(unittest.TestCase):
    def test_lock_requires_complete_commit_hashes_icex_forks_and_safe_paths(self):
        original=json.loads((ROOT/'sources.lock.json').read_text())
        for change in ('branch','url','path','missing'):
            with self.subTest(change=change),tempfile.TemporaryDirectory() as tmp:
                data=copy.deepcopy(original)
                if change=='branch':data['components']['wine']['commit']='main'
                if change=='url':data['components']['wine']['repository']='https://evil.invalid/wine.git'
                if change=='path':data['components']['wine']['modified_paths']=['../outside']
                if change=='missing':del data['components']['wine']
                path=Path(tmp)/'lock.json';path.write_text(json.dumps(data))
                with self.assertRaises(ValueError):pins_module.load(path)
    def test_packages_pin_an_exact_commit_and_version(self):
        pkgs=pins_module.load_packages()
        self.assertEqual(pkgs['dlssnr-amd']['commit'],'9503f6b180df45305b38fbd21cd9f705164f2ab1')
        self.assertEqual(pkgs['dlssnr-amd']['version'],'0.0.9')
        original=json.loads((ROOT/'sources.lock.json').read_text())
        for change in ('branch','version','url','extra','missing'):
            with self.subTest(change=change),tempfile.TemporaryDirectory() as tmp:
                data=copy.deepcopy(original)
                if change=='branch':data['packages']['dlssnr-amd']['commit']='main'
                if change=='version':data['packages']['dlssnr-amd']['version']='latest'
                if change=='url':data['packages']['dlssnr-amd']['repository']='http://x/y.git'
                if change=='extra':data['packages']['other']=dict(data['packages']['dlssnr-amd'])
                if change=='missing':del data['packages']
                path=Path(tmp)/'lock.json';path.write_text(json.dumps(data))
                with self.assertRaises(ValueError):pins_module.load_packages(path)
    def test_components_still_require_exactly_the_five_forks(self):
        self.assertEqual(set(pins_module.load()),pins_module.NAMES)
    def test_the_package_command_prints_one_field(self):
        out=subprocess.check_output(['python3','scripts/source-pins.py','package','dlssnr-amd','commit'],cwd=ROOT,text=True)
        self.assertEqual(out.strip(),'9503f6b180df45305b38fbd21cd9f705164f2ab1')
    def test_a_wrong_checkout_or_nested_gitlink_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo=Path(tmp)
            def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],text=True).strip()
            git('init','--quiet');git('config','user.name','Test');git('config','user.email','test@example.invalid')
            (repo/'.gitmodules').write_text('[submodule "wine"]\npath = wine\nurl = https://github.com/icex/wine.git\n[submodule "vkd3d-proton"]\npath = vkd3d-proton\nurl = https://github.com/icex/vkd3d-proton.git\n')
            git('add','.gitmodules')
            git('update-index','--add','--cacheinfo','160000,'+'a'*40+',wine')
            git('update-index','--add','--cacheinfo','160000,'+'b'*40+',vkd3d-proton')
            git('commit','--quiet','-m','fixture')
            pins=pins_module.load();pins=copy.deepcopy(pins)
            pins['proton']['commit']=git('rev-parse','HEAD');pins['wine']['commit']='a'*40;pins['vkd3d-proton']['commit']='b'*40
            pins_module.verify_graph(pins,{'proton':repo})
            pins['wine']['commit']='c'*40
            with self.assertRaisesRegex(ValueError,'gitlink'):pins_module.verify_graph(pins,{'proton':repo})
            pins['proton']['commit']='d'*40
            with self.assertRaisesRegex(ValueError,'checkout'):pins_module.verify_graph(pins,{'proton':repo})
    def test_modified_tracked_sources_are_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo=Path(tmp)
            def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],text=True).strip()
            git('init','--quiet');git('config','user.name','Test');git('config','user.email','test@example.invalid')
            source=repo/'source.c';source.write_text('original')
            git('add','source.c');git('commit','--quiet','-m','fixture')
            pins=copy.deepcopy(pins_module.load());pins['xodus']['commit']=git('rev-parse','HEAD')
            pins_module.verify_graph(pins,{'xodus':repo},require_clean=True)
            source.write_text('modified')
            with self.assertRaisesRegex(ValueError,'uncommitted'):
                pins_module.verify_graph(pins,{'xodus':repo},require_clean=True)
            git('add','source.c')
            with self.assertRaisesRegex(ValueError,'uncommitted'):
                pins_module.verify_graph(pins,{'xodus':repo},require_clean=True)
    def test_launcher_repository_must_match_the_lock(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo=Path(tmp)
            def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],text=True).strip()
            git('init','--quiet');pins=pins_module.load()
            for name,path in [('xodus','third_party/xodus-cli'),('proton','third_party/xodus-proton')]:
                git('update-index','--add','--cacheinfo','160000,'+pins[name]['commit']+','+path)
                git('config','-f','.gitmodules','submodule.'+path+'.url',pins[name]['repository'])
            pins_module.verify_launcher(pins,repo)
            git('config','-f','.gitmodules','submodule.third_party/xodus-cli.url','https://example.invalid/other.git')
            with self.assertRaisesRegex(ValueError,'repository'):
                pins_module.verify_launcher(pins,repo)
    def test_aur_and_source_lock_pin_the_same_client(self):
        pins=pins_module.load()
        recipe=(ROOT/'packaging/aur/ferestre-client/PKGBUILD').read_text()
        commit=re.search(r'^_commit=([0-9a-f]{40})$',recipe,re.M)[1]
        self.assertEqual(commit,pins['xodus']['commit'])
        info=(ROOT/'packaging/aur/ferestre-client/.SRCINFO').read_text()
        self.assertIn('https://github.com/icex/xodus.git#commit='+commit,info)
        self.assertNotIn('.patch',info)
    def test_build_paths_use_fork_sources_without_patch_application(self):
        for name in ('.github/workflows/runtime.yml','.github/workflows/release.yml','.github/workflows/rust.yml','scripts/install-xodus-proton.sh','packaging/aur/ferestre-client/PKGBUILD'):
            text='\n'.join(line for line in (ROOT/name).read_text().splitlines() if not line.lstrip().startswith('#'))
            self.assertNotRegex(text,r'\bgit\s+(?:-C\s+\S+\s+)?apply\b|\bpatch\s+-p\d')
        self.assertFalse((ROOT/'patches').exists(),'component fixes belong in the forks')
        for name in ('packaging/appimage/build-appimage.sh','.github/workflows/release.yml','packaging/aur/ferestre-git/PKGBUILD'):
            self.assertIn('sources.lock.json',(ROOT/name).read_text())

if __name__=='__main__':unittest.main()
