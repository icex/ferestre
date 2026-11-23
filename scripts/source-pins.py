#!/usr/bin/env python3
"""Read and verify the exact source commits used by builds and packages."""
import argparse
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
NAMES = {'proton', 'wine', 'xgameruntime', 'vkd3d-proton', 'dxvk', 'xodus'}
# Third-party packages Ferestre builds and hosts from an exact upstream commit.
# Not forks, so they are validated apart from `components`.
PACKAGES = {'dlssnr-amd'}


def load(path=ROOT / 'sources.lock.json'):
    data = json.loads(path.read_text())
    if data.get('schema') != 1 or set(data.get('components', {})) != NAMES:
        raise ValueError('source lock must name all six components')
    for name, pin in data['components'].items():
        if not re.fullmatch(r'https://github\.com/icex/[A-Za-z0-9_.-]+\.git', pin['repository']):
            raise ValueError(name + ': expected an icex fork URL')
        for key in ('commit', 'upstream_commit'):
            if not re.fullmatch('[0-9a-f]{40}', pin[key]):
                raise ValueError(name + ': pin must be a complete commit hash')
        for entry in pin['modified_paths']:
            if not entry or PurePosixPath(entry).is_absolute() or '..' in PurePosixPath(entry).parts:
                raise ValueError(name + ': invalid source path')
    return data['components']


def load_packages(path=ROOT / 'sources.lock.json'):
    data = json.loads(path.read_text())
    packages = data.get('packages')
    if not isinstance(packages, dict) or set(packages) != PACKAGES:
        raise ValueError('source lock must name exactly the packages ' + ', '.join(sorted(PACKAGES)))
    for name, pin in packages.items():
        if not isinstance(pin, dict) or set(pin) != {'repository', 'commit', 'version'}:
            raise ValueError(name + ': a package pin has exactly repository, commit and version')
        if not re.fullmatch(r'https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\.git', str(pin['repository'])):
            raise ValueError(name + ': expected a GitHub repository URL')
        if not re.fullmatch('[0-9a-f]{40}', str(pin['commit'])):
            raise ValueError(name + ': pin must be a complete commit hash')
        if not re.fullmatch(r'\d+(\.\d+){2,3}', str(pin['version'])):
            raise ValueError(name + ': version must be a release number')
    return packages


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()


def verify_graph(pins, repos, require_clean=False):
    for name, repo in repos.items():
        if git(repo, 'rev-parse', 'HEAD') != pins[name]['commit']:
            raise ValueError(name + ': checkout does not match the locked commit')
        if require_clean and subprocess.run(['git','-C',str(repo),'diff','--quiet','--ignore-submodules=dirty','HEAD']).returncode:
            raise ValueError(name + ': source checkout contains uncommitted changes')
    for parent, sub, child in [('proton', 'wine', 'wine'), ('proton', 'vkd3d-proton', 'vkd3d-proton'),
                               ('proton', 'dxvk', 'dxvk'),
                               ('wine', 'dlls/xgameruntime', 'xgameruntime')]:
        if parent not in repos:
            continue
        repo, pin = repos[parent], pins[child]
        entry = git(repo, 'ls-tree', pins[parent]['commit'], '--', sub).split()
        if len(entry) < 3 or entry[0] != '160000' or entry[2] != pin['commit']:
            raise ValueError(parent + ': mismatched ' + sub + ' gitlink')
        url = git(repo, 'config', '--blob', pins[parent]['commit'] + ':.gitmodules',
                  '--get', 'submodule.' + sub + '.url')
        if url != pin['repository']:
            raise ValueError(parent + ': mismatched ' + sub + ' repository')


def verify_launcher(pins, root=ROOT):
    for name, path in [('xodus', 'third_party/xodus-cli'), ('proton', 'third_party/xodus-proton')]:
        entry = git(root, 'ls-files', '--stage', '--', path).split()
        if len(entry) < 2 or entry[0] != '160000' or entry[1] != pins[name]['commit']:
            raise ValueError(path + ': gitlink differs from the source lock')
        url = git(root, 'config', '-f', '.gitmodules', '--get', 'submodule.' + path + '.url')
        if url != pins[name]['repository']:
            raise ValueError(path + ': repository differs from the source lock')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['get', 'modified', 'verify', 'verify-remote', 'package'])
    parser.add_argument('component', nargs='?', choices=sorted(NAMES | PACKAGES))
    parser.add_argument('field', nargs='?', choices=['repository', 'commit', 'upstream_commit', 'version'])
    parser.add_argument('--directory', type=Path)
    args = parser.parse_args()
    if args.command == 'package':
        if args.component not in PACKAGES:
            parser.error('package is one of: ' + ', '.join(sorted(PACKAGES)))
        if args.field not in ('repository', 'commit', 'version'):
            parser.error('field is one of: repository, commit, version')
        print(load_packages()[args.component][args.field])
        return
    if args.component is not None and args.component not in NAMES:
        parser.error('component is one of: ' + ', '.join(sorted(NAMES)))
    if args.field == 'version':
        parser.error('version is a package field')
    pins = load()
    if args.command in ('get', 'modified'):
        if not args.component:
            parser.error('component is required')
        if args.command == 'modified':
            print('\n'.join(pins[args.component]['modified_paths']))
        elif not args.field:
            parser.error('field is required')
        else:
            print(pins[args.component][args.field])
    elif args.command == 'verify':
        if args.component:
            if args.directory is None:
                parser.error('--directory is required')
            verify_graph(pins, {args.component: args.directory}, require_clean=True)
        else:
            verify_launcher(pins)
            print('Source lock and launcher gitlinks agree')
    else:
        # Only the commit and .gitmodules blobs are needed, not a Wine checkout.
        with tempfile.TemporaryDirectory(prefix='ferestre-source-check-') as tmp:
            repos = {}
            for name, pin in pins.items():
                repo = Path(tmp) / name
                subprocess.run(['git', 'init', '--quiet', str(repo)], check=True)
                subprocess.run(['git', '-C', str(repo), 'remote', 'add', 'origin', pin['repository']], check=True)
                subprocess.run(['git', '-C', str(repo), 'fetch', '--quiet', '--depth=1', '--filter=blob:none', 'origin', pin['commit']], check=True)
                subprocess.run(['git', '-C', str(repo), 'update-ref', 'HEAD', pin['commit']], check=True)
                repos[name] = repo
            verify_graph(pins, repos)
            print('All six public fork commits and nested source pins verified')


if __name__ == '__main__':
    main()
