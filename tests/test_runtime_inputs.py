#!/usr/bin/env python3
"""The runtime inputs digest and the rules for promoting a main build to a tag.

scripts/runtime-inputs.py decides whether a runtime-* tag ships a dist that a
push to main compiled, instead of compiling it again. A wrong yes would sign
something built from unreviewed sources, so every way a candidate can be
unacceptable is tried here, against API responses recorded from this
repository (tests/fixtures/runtime-inputs/api.json) and variations of them.
Offline: nothing is fetched.
"""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

REPO = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('runtime_inputs', REPO / 'scripts/runtime-inputs.py')
ri = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ri)

API = json.loads((REPO / 'tests/fixtures/runtime-inputs/api.json').read_text())
REPO_NAME = 'icex/ferestre'
LOCK = (REPO / 'sources.lock.json').read_text()
BLOB = 'a' * 40
D = ri.digest_of(ri.inputs(LOCK, BLOB))
TAG = 'f' * 40
MAIN_RUN = 42404010296      # a push to main whose build succeeded (hosted, Nov 7)
TAG_RUN = 44962707713       # a runtime-* tag build, with its artefact
CANCELLED_MAIN = 44962698296


def recorded_run(rid):
    return copy.deepcopy(next(r for r in API['runs'] if r['id'] == rid))


def good_candidate(rid=MAIN_RUN, digest=D):
    """The recorded main run, with the artefact a new-style build uploads."""
    run = recorded_run(rid)
    art = {'id': 9000 + rid % 1000, 'name': ri.artifact_name(digest), 'expired': False,
           'digest': 'sha256:' + 'b' * 64,
           'workflow_run': {'id': run['id'], 'head_sha': run['head_sha'], 'head_branch': 'main'}}
    return run, art


class Fake:
    def __init__(self, runs, jobs=None, artifacts=None, digest_at=None, ancestor=None):
        self.runs = runs
        self.jobs = {int(k): v for k, v in API['jobs'].items()}
        self.jobs.update(jobs or {})
        self.artifacts = {int(k): v for k, v in API['artifacts'].items()}
        self.artifacts.update(artifacts or {})
        self.digest_at = digest_at or (lambda sha: D)
        self.ancestor = ancestor or (lambda a, b: True)
        self.log = []

    def select(self, digest=D):
        return ri.select(self.runs, lambda r: self.jobs.get(r, []), lambda r: self.artifacts.get(r, []),
                         repo=REPO_NAME, digest=digest, tag_sha=TAG, digest_at=self.digest_at,
                         is_ancestor=self.ancestor, log=self.log.append)


class DigestTests(unittest.TestCase):
    def lock_with(self, edit):
        data = json.loads(LOCK)
        edit(data)
        return json.dumps(data)

    def test_the_digest_is_canonical(self):
        reordered = json.dumps(json.loads(LOCK), sort_keys=True, indent=4)
        self.assertEqual(ri.digest_of(ri.inputs(reordered, BLOB)), D)
        self.assertRegex(D, r'^[0-9a-f]{64}$')

    def test_lock_edits_outside_the_runtime_pins_keep_the_digest(self):
        for edit in (lambda d: d['packages']['dlssnr-amd'].update(commit='0' * 40),
                     lambda d: d['components']['xodus'].update(commit='0' * 40),
                     lambda d: d['components']['wine']['modified_paths'].append('x'),
                     lambda d: d['components']['proton'].update(upstream_commit='0' * 40)):
            self.assertEqual(ri.digest_of(ri.inputs(self.lock_with(edit), BLOB)), D)

    def test_every_runtime_pin_and_the_build_script_change_it(self):
        for name in ri.COMPONENTS:
            for key, value in (('commit', '0' * 40), ('repository', 'https://github.com/icex/x.git')):
                lock = self.lock_with(lambda d: d['components'][name].update({key: value}))
                self.assertNotEqual(ri.digest_of(ri.inputs(lock, BLOB)), D, f'{name}.{key}')
        self.assertNotEqual(ri.digest_of(ri.inputs(LOCK, 'c' * 40)), D)

    def test_short_or_missing_commits_are_refused(self):
        with self.assertRaises(ValueError):
            ri.inputs(self.lock_with(lambda d: d['components']['wine'].update(commit='abc')), BLOB)
        with self.assertRaises(ValueError):
            ri.inputs(LOCK, None)

    def test_pull_request_43_kept_the_runtime_inputs_of_40(self):
        # 9bfc719 (#43) only moved the dlssnr pin; bfac45c is #40.
        for rev in ('9bfc719', 'bfac45c'):
            if subprocess.run(['git', '-C', str(REPO), 'cat-file', '-e', rev + '^{commit}'],
                              capture_output=True).returncode:
                self.skipTest('history not available')
        at = lambda rev: ri.digest_of(ri.inputs(ri.git('show', f'{rev}:sources.lock.json'), BLOB))
        self.assertEqual(at('9bfc719'), at('bfac45c'))

    def test_the_working_tree_digest_matches_the_cli(self):
        out = subprocess.run([sys.executable, str(REPO / 'scripts/runtime-inputs.py'), 'digest'],
                             capture_output=True, text=True, check=True).stdout.split()
        self.assertEqual(out, [ri.digest_at(None)])


class PromotionTests(unittest.TestCase):
    def test_nothing_recorded_today_is_promotable(self):
        # Tags, manual runs and the cancelled main builds: all refused, so the
        # first tag after this change builds.
        fake = Fake([recorded_run(r['id']) for r in API['runs']])
        self.assertEqual(fake.select()[0], 'build')
        reasons = '\n'.join(fake.log)
        self.assertIn(f'run {TAG_RUN}: branch runtime-11.0.20261122.1', reasons)
        self.assertIn(f'run {CANCELLED_MAIN}: build cancelled', reasons)
        self.assertIn('event workflow_dispatch', reasons)
        # The recorded main run that did succeed has no new-style artefact.
        self.assertIn(f'run {MAIN_RUN}: 0 artefacts named runtime-dist-', reasons)

    def test_the_recorded_tag_artefact_is_refused(self):
        run = recorded_run(TAG_RUN)
        art = copy.deepcopy(API['artifacts'][str(TAG_RUN)][0])
        art['name'] = ri.artifact_name(D)
        fake = Fake([run], artifacts={TAG_RUN: [art]})
        self.assertEqual(fake.select()[0], 'build')
        self.assertIn('branch runtime-', fake.log[0])

    def test_a_good_main_build_is_promoted(self):
        run, art = good_candidate()
        verdict, got, artefact = Fake([run], artifacts={MAIN_RUN: [art]}).select()
        self.assertEqual((verdict, got['id'], artefact['id']), ('promote', MAIN_RUN, art['id']))

    def test_the_newest_good_build_wins(self):
        old, old_art = good_candidate()
        new = copy.deepcopy(old)
        new.update(id=old['id'] + 1, run_number=old['run_number'] + 5)
        new_art = dict(old_art, id=old_art['id'] + 1,
                       workflow_run=dict(old_art['workflow_run'], id=new['id']))
        fake = Fake([old, new], jobs={new['id']: API['jobs'][str(MAIN_RUN)]},
                    artifacts={MAIN_RUN: [old_art], new['id']: [new_art]})
        self.assertEqual(fake.select()[1]['id'], new['id'])

    def refused(self, mutate_run=None, mutate_art=None, jobs=None, **fake):
        run, art = good_candidate()
        if mutate_run:
            mutate_run(run)
        arts = [art]
        if mutate_art:
            arts = mutate_art(art) or arts
        f = Fake([run], jobs={MAIN_RUN: jobs} if jobs is not None else None,
                 artifacts={MAIN_RUN: arts}, **fake)
        verdict = f.select()
        self.assertEqual(verdict[0], 'build', f.log)
        return f.log[0]

    def test_other_events_branches_workflows_and_repositories_are_refused(self):
        cases = {
            'event pull_request': lambda r: r.update(event='pull_request'),
            'event workflow_dispatch': lambda r: r.update(event='workflow_dispatch'),
            'event schedule': lambda r: r.update(event='schedule'),
            'branch dev': lambda r: r.update(head_branch='dev'),
            'branch runtime-1': lambda r: r.update(head_branch='runtime-1'),
            'workflow .github/workflows/rust.yml': lambda r: r.update(path='.github/workflows/rust.yml'),
            'repository someone/ferestre': lambda r: r['repository'].update(full_name='someone/ferestre'),
            'head_repository someone/ferestre': lambda r: r['head_repository'].update(full_name='someone/ferestre'),
            'no head commit': lambda r: r.update(head_sha='abc'),
        }
        for expect, mutate in cases.items():
            self.assertIn(expect, self.refused(mutate), expect)

    def test_a_build_that_did_not_succeed_is_refused(self):
        for conclusion in ('failure', 'cancelled', 'skipped', None):
            jobs = [{'name': 'build', 'status': 'completed', 'conclusion': conclusion}]
            self.assertIn(f'build {conclusion}', self.refused(jobs=jobs))
        self.assertIn('no single build job', self.refused(jobs=[]))
        twice = [{'name': 'build', 'conclusion': 'success'}] * 2
        self.assertIn('no single build job', self.refused(jobs=twice))

    def test_unusable_artefacts_are_refused(self):
        self.assertIn('expired', self.refused(mutate_art=lambda a: a.update(expired=True)))
        self.assertIn('no digest', self.refused(mutate_art=lambda a: a.update(digest=None)))
        self.assertIn('no digest', self.refused(mutate_art=lambda a: a.update(digest='md5:00')))
        self.assertIn('0 artefacts', self.refused(
            mutate_art=lambda a: a.update(name=ri.artifact_name('e' * 64))))
        self.assertIn('0 artefacts', self.refused(
            mutate_art=lambda a: a.update(name='runtime-dist-run-1')))
        self.assertIn('2 artefacts', self.refused(mutate_art=lambda a: [a, dict(a, id=1)]))
        self.assertIn('another run', self.refused(
            mutate_art=lambda a: a['workflow_run'].update(id=1)))
        self.assertIn('another run', self.refused(
            mutate_art=lambda a: a['workflow_run'].update(head_sha='0' * 40)))

    def test_a_commit_that_is_not_behind_the_tag_is_refused(self):
        self.assertIn('not an ancestor', self.refused(ancestor=lambda a, b: False))

    def test_a_commit_with_other_inputs_is_refused(self):
        # The digest is recomputed from the candidate commit's own files, not
        # read from the artefact's name.
        self.assertIn('other runtime inputs', self.refused(digest_at=lambda sha: 'e' * 64))
        self.assertIn('other runtime inputs', self.refused(digest_at=lambda sha: None))

    def test_a_running_main_build_of_the_same_inputs_is_waited_for(self):
        run, _ = good_candidate()
        run.update(status='in_progress', conclusion=None)
        self.assertEqual(Fake([run]).select()[0], 'wait')
        # ...but a finished one with the same inputs is used at once.
        older, art = good_candidate()
        older.update(id=older['id'] - 1, run_number=older['run_number'] - 1)
        art['workflow_run']['id'] = older['id']
        fake = Fake([run, older], jobs={older['id']: API['jobs'][str(MAIN_RUN)]},
                    artifacts={older['id']: [art]})
        self.assertEqual(fake.select()[:2], ('promote', older))
        # A running build of other inputs is not waited for.
        self.assertEqual(Fake([run], digest_at=lambda s: 'e' * 64).select()[0], 'build')


def manifest(**over):
    m = {'schema': 'ferestre-runtime-dist/1', 'digest': D, 'inputs': ri.inputs(LOCK, BLOB),
         'sdk_image': 'registry.example/sdk:1', 'head_sha': '1' * 40, 'ref': 'refs/heads/main',
         'event': 'push', 'run_id': 7, 'run_attempt': 1, 'repository': REPO_NAME,
         'built_proton': json.loads(LOCK)['components']['proton']['commit'],
         'dist_sha256': '2' * 64}
    m.update(over)
    return m


class ManifestTests(unittest.TestCase):
    ARGS = dict(digest=D, run_id=7, head_sha='1' * 40, ref='refs/heads/main', repo=REPO_NAME,
                dist_sha256='2' * 64, expect_proton=json.loads(LOCK)['components']['proton']['commit'])

    def test_a_matching_manifest_passes(self):
        self.assertEqual(ri.check_manifest(manifest(), **self.ARGS), [])

    def test_every_field_is_checked(self):
        bad = {
            'schema': 'other', 'digest': 'e' * 64, 'run_id': 8, 'head_sha': '3' * 40,
            'ref': 'refs/heads/dev', 'event': 'workflow_dispatch', 'repository': 'someone/ferestre',
            'dist_sha256': '4' * 64, 'built_proton': '5' * 40,
            'inputs': ri.inputs(LOCK, 'c' * 40),
        }
        for key, value in bad.items():
            self.assertNotEqual(ri.check_manifest(manifest(**{key: value}), **self.ARGS), [], key)

    def test_a_manual_build_of_another_proton_passes_only_unpinned(self):
        m = manifest(built_proton='5' * 40, ref='refs/heads/x', event='workflow_dispatch')
        args = dict(self.ARGS, ref='refs/heads/x')
        self.assertNotEqual(ri.check_manifest(m, **args), [])
        self.assertEqual(ri.check_manifest(m, **dict(args, expect_proton=None)), [])


class CliTests(unittest.TestCase):
    def run_cli(self, *args, env=None):
        return subprocess.run([sys.executable, str(REPO / 'scripts/runtime-inputs.py'), *args],
                              capture_output=True, text=True, env=env)

    def test_manifest_then_check_round_trip(self):
        digest = ri.digest_at(None)
        if digest is None:
            self.skipTest('no build script in this checkout')
        pinned = json.loads(LOCK)['components']['proton']['commit']
        with tempfile.TemporaryDirectory() as tmp:
            t = Path(tmp)
            (t / 'dist.tar.zst').write_bytes(b'not really a dist')
            env = dict(os.environ, GITHUB_SHA='1' * 40, GITHUB_REF='refs/heads/main',
                       GITHUB_EVENT_NAME='push', GITHUB_RUN_ID='42', GITHUB_REPOSITORY=REPO_NAME)
            r = self.run_cli('manifest', '--out', str(t / 'manifest.json'), '--digest', digest,
                             '--sdk', 'img', '--dist', str(t / 'dist.tar.zst'),
                             '--built-proton', pinned, env=env)
            self.assertEqual(r.returncode, 0, r.stderr)
            with zipfile.ZipFile(t / 'a.zip', 'w') as z:
                z.write(t / 'dist.tar.zst', 'dist.tar.zst')
                z.write(t / 'manifest.json', 'manifest.json')
            zip_digest = 'sha256:' + hashlib.sha256((t / 'a.zip').read_bytes()).hexdigest()
            common = ['check', '--zip', str(t / 'a.zip'), '--manifest', str(t / 'manifest.json'),
                      '--dist', str(t / 'dist.tar.zst'), '--digest', digest, '--run-id', '42',
                      '--head-sha', '1' * 40, '--repo', REPO_NAME, '--ref', 'refs/heads/main',
                      '--expect-proton', pinned]
            ok = self.run_cli(*common, '--api-digest', zip_digest)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            # A zip that is not the one GitHub recorded.
            bad = self.run_cli(*common, '--api-digest', 'sha256:' + '0' * 64)
            self.assertNotEqual(bad.returncode, 0)
            self.assertIn('GitHub recorded', bad.stderr)
            self.assertNotEqual(self.run_cli(*common, '--api-digest', '').returncode, 0)
            # The right zip, but from another run.
            other = self.run_cli(*[a if a != '42' else '43' for a in common], '--api-digest', zip_digest)
            self.assertIn('not 43', other.stderr)
            # A different dist beside the right manifest.
            (t / 'dist.tar.zst').write_bytes(b'swapped')
            swapped = self.run_cli(*common, '--api-digest', zip_digest)
            self.assertIn('dist.tar.zst is not the file', swapped.stderr)

    def test_manifest_refuses_a_digest_that_is_not_the_checkouts(self):
        r = self.run_cli('manifest', '--out', os.devnull, '--digest', 'e' * 64, '--sdk', 'x',
                         '--dist', os.devnull, '--built-proton', '0' * 40,
                         env=dict(os.environ, GITHUB_SHA='1' * 40, GITHUB_REF='r',
                                  GITHUB_EVENT_NAME='push', GITHUB_RUN_ID='1', GITHUB_REPOSITORY='a/b'))
        self.assertNotEqual(r.returncode, 0)


if __name__ == '__main__':
    unittest.main()
