#!/usr/bin/env python3
"""Which Rust CI jobs a pull request runs (.github/scripts/ci-changes.sh).

A path the `sources` or `gui` job reads but the filter leaves out would let a
pull request that breaks that job merge green, so every repository path those
jobs name is checked to switch them on. Runs the script against throwaway git
repositories; nothing is fetched.
"""
from pathlib import Path
import json
import os
import re
import subprocess
import tempfile
import unittest

import yaml

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / '.github/scripts/ci-changes.sh'
RUST = REPO / '.github/workflows/rust.yml'


def decide(event, changed=()):
    """Run the script on a repository whose last commit touches `changed`."""
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t',
                   GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t',
                   GITHUB_OUTPUT=str(Path(tmp) / 'out'))
        repo = Path(tmp) / 'repo'
        git = lambda *a: subprocess.run(['git', '-C', str(repo), *a], env=env, check=True,
                                        capture_output=True)
        subprocess.run(['git', 'init', '-q', str(repo)], check=True)
        (repo / 'README').write_text('base\n')
        git('add', '-A')
        git('commit', '-qm', 'base')
        for path in changed:
            (repo / path).parent.mkdir(parents=True, exist_ok=True)
            (repo / path).write_text('changed\n')
        (repo / 'README').write_text('head\n')
        git('add', '-A')
        git('commit', '-qm', 'head')
        subprocess.run([str(SCRIPT), event], cwd=repo, env=env, check=True,
                       capture_output=True)
        lines = (Path(tmp) / 'out').read_text().split()
        return dict(line.split('=', 1) for line in lines)


def job_paths(job):
    """Repository files a job's shell steps name."""
    text = '\n'.join(s.get('run', '') for s in job['steps'])
    found = set(re.findall(r'\b((?:tests|scripts)/[\w.-]+\.(?:py|sh))\b', text))
    return sorted(p for p in found if (REPO / p).exists())


class CiChangesTests(unittest.TestCase):
    def test_pushes_and_manual_runs_run_everything(self):
        for event in ('push', 'workflow_dispatch'):
            self.assertEqual(decide(event), {'rust': 'true', 'gui': 'true', 'sources': 'true'})

    def test_a_docs_only_pull_request_runs_none_of_them(self):
        self.assertEqual(decide('pull_request', ['docs/TESTING.md']),
                         {'rust': 'false', 'gui': 'false', 'sources': 'false'})

    def test_a_crate_change_runs_msrv_and_gui_but_not_sources(self):
        self.assertEqual(decide('pull_request', ['crates/ferestre-core/src/lib.rs']),
                         {'rust': 'true', 'gui': 'true', 'sources': 'false'})

    def test_the_workflow_and_its_helpers_run_everything(self):
        for path in ('.github/workflows/rust.yml', '.github/scripts/ci-target-dir.sh'):
            self.assertEqual(decide('pull_request', [path]),
                             {'rust': 'true', 'gui': 'true', 'sources': 'true'}, path)

    def test_every_file_the_sources_job_names_switches_it_on(self):
        job = yaml.safe_load(RUST.read_text())['jobs']['sources']
        paths = job_paths(job)
        self.assertIn('tests/run-tests.sh', paths)
        for path in paths + ['sources.lock.json', '.gitmodules', 'third_party/xodus-cli/x',
                             'scripts/check-sources.sh']:
            self.assertEqual(decide('pull_request', [path])['sources'], 'true', path)

    def test_every_file_the_gui_jobs_name_switches_them_on(self):
        jobs = yaml.safe_load(RUST.read_text())['jobs']
        paths = job_paths(jobs['gui-build']) + job_paths(jobs['gui'])
        self.assertIn('tests/run_gui_ci.py', paths)
        self.assertIn('tests/test_gui_actions.py', paths)
        # The runner reads its tests from the manifest.
        manifest = json.loads((REPO / 'tests/gui_ci.json').read_text())['tests']
        scripts = sorted({t['script'] for t in manifest})
        self.assertIn('tests/gui_smoke.py', scripts)
        for path in paths + scripts + ['tests/gui_ci.json', 'tests/controller_driver.py',
                                       'titles/a.toml', 'Cargo.lock']:
            self.assertEqual(decide('pull_request', [path])['gui'], 'true', path)


if __name__ == '__main__':
    unittest.main()
