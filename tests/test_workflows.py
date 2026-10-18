#!/usr/bin/env python3
"""What the CI and release workflows must keep true.

The release workflows hold a token that can rewrite releases and, on a tag, the
key that signs them. Most of what keeps those safe is a line of YAML that is
easy to drop in an unrelated edit, so it is checked here rather than trusted to
review. Static: nothing is run and nothing is fetched.
"""
from pathlib import Path
import re
import subprocess
import unittest

import yaml

REPO = Path(__file__).resolve().parents[1]
WORKFLOWS = sorted((REPO / '.github/workflows').glob('*.yml'))
PINNED = re.compile(r'^[\w.-]+/[\w.-]+(/[\w./-]+)?@[0-9a-f]{40}$')


def load(path):
    return yaml.safe_load(path.read_text())


def jobs():
    for path in WORKFLOWS:
        for name, job in (load(path).get('jobs') or {}).items():
            yield path.name, name, job


def runs(job):
    """Each step's shell text, with backslash continuations joined."""
    for step in job.get('steps', []):
        if 'run' in step:
            yield re.sub(r'\\\n\s*', ' ', step['run'])


def can_write(job, workflow):
    perms = job.get('permissions', workflow.get('permissions', {}))
    if isinstance(perms, str):
        return perms == 'write-all'
    return any(v == 'write' for v in (perms or {}).values())


class WorkflowTests(unittest.TestCase):
    def test_every_workflow_defaults_to_a_read_only_token(self):
        for path in WORKFLOWS:
            self.assertEqual(load(path).get('permissions'), {'contents': 'read'}, path.name)

    def test_actions_are_pinned_to_commits(self):
        for wf, name, job in jobs():
            for step in job.get('steps', []):
                uses = step.get('uses')
                if uses and not uses.startswith('./'):
                    self.assertRegex(uses, PINNED, f'{wf}/{name}: {uses} can be moved')

    def test_checkouts_do_not_keep_the_token(self):
        for wf, name, job in jobs():
            for step in job.get('steps', []):
                if str(step.get('uses', '')).startswith('actions/checkout@'):
                    self.assertIs(step.get('with', {}).get('persist-credentials'), False,
                                  f'{wf}/{name} leaves the token in .git/config')

    def test_jobs_that_can_write_build_nothing(self):
        building = re.compile(r'\b(cargo|make|configure\.sh|build-appimage\.sh|rustup)\b')
        for path in WORKFLOWS:
            workflow = load(path)
            for name, job in workflow.get('jobs', {}).items():
                if not can_write(job, workflow):
                    continue
                for text in runs(job):
                    self.assertNotRegex(text, building, f'{path.name}/{name}')
                for step in job.get('steps', []):
                    uses = str(step.get('uses', ''))
                    self.assertTrue(uses == '' or uses.startswith(
                        ('actions/checkout@', 'actions/download-artifact@')),
                        f'{path.name}/{name} runs {uses} with a write token')

    def test_the_signing_key_reaches_only_the_publishing_jobs(self):
        for path in WORKFLOWS:
            workflow = load(path)
            for name, job in workflow.get('jobs', {}).items():
                if 'secrets.MINISIGN' in yaml.safe_dump(job):
                    self.assertTrue(can_write(job, workflow), f'{path.name}/{name}')
                    self.assertEqual(job.get('needs'), 'build', f'{path.name}/{name}')
                    # Environment secrets, so the environment's tag rule and
                    # required reviewer gate every job that can read them.
                    self.assertEqual(job.get('environment'), 'release', f'{path.name}/{name}')
                    self.assertEqual(job.get('if'), "github.ref_type == 'tag'",
                                     f'{path.name}/{name}')
                    for step in job.get('steps', []):
                        if 'secrets.MINISIGN' in yaml.safe_dump(step):
                            self.assertEqual(step.get('if'), "github.ref_type == 'tag'",
                                             f'{path.name}/{name}/{step.get("name")}')

    def test_releases_are_signed_before_they_are_published(self):
        for wf in ('release.yml', 'runtime.yml'):
            workflow = load(REPO / '.github/workflows' / wf)
            publishing = [(n, j) for n, j in workflow['jobs'].items()
                          if any('gh release create' in t for t in runs(j))]
            self.assertEqual([n for n, _ in publishing], ['publish'], wf)
            job = publishing[0][1]
            self.assertEqual(job.get('if'), "github.ref_type == 'tag'", wf)
            texts = list(runs(job))
            sign = next(i for i, t in enumerate(texts) if 'packaging/sign-release.sh out' in t)
            publish = next(i for i, t in enumerate(texts) if 'gh release create' in t)
            self.assertLess(sign, publish, wf)
            self.assertIn('out/*', texts[publish], wf)
            self.assertFalse(can_write(workflow['jobs']['build'], workflow), wf)

    def test_signatures_carry_the_tags_version_and_manual_runs_say_they_are_unsigned(self):
        for wf, prefix in (('release.yml', 'v'), ('runtime.yml', 'runtime-')):
            workflow = load(REPO / '.github/workflows' / wf)
            sign = next(t for t in runs(workflow['jobs']['publish'])
                        if 'packaging/sign-release.sh out' in t)
            self.assertIn(f'tag_version=${{GITHUB_REF_NAME#{prefix}}}', sign, wf)
            self.assertIn('packaging/sign-release.sh out "$tag_version"', sign, wf)
            unsigned = [s for s in workflow['jobs']['build']['steps']
                        if s.get('name') == 'Unsigned build']
            self.assertEqual(len(unsigned), 1, wf)
            self.assertEqual(unsigned[0].get('if'), "github.ref_type != 'tag'", wf)
            self.assertIn('UNSIGNED', unsigned[0]['run'], wf)
            self.assertIn('GITHUB_STEP_SUMMARY', unsigned[0]['run'], wf)

    def test_the_msrv_job_builds_with_the_declared_rust_version(self):
        declared = re.search(r'^rust-version = "([0-9.]+)"$',
                             (REPO / 'Cargo.toml').read_text(), re.M).group(1)
        job = load(REPO / '.github/workflows/rust.yml')['jobs']['msrv']
        toolchains = [s['with']['toolchain'] for s in job['steps']
                      if str(s.get('uses', '')).startswith('dtolnay/rust-toolchain@')]
        self.assertEqual(toolchains, [declared])
        self.assertTrue(any(f'cargo +{declared} test --locked --workspace' in t
                            for t in runs(job)))

    def test_the_published_launcher_must_trust_the_signing_key(self):
        job = load(REPO / '.github/workflows/release.yml')['jobs']['publish']
        self.assertTrue(any('FERESTRE_EXPECT_PUBKEY_IN=' in t and 'usr/bin/ferestre' in t
                            for t in runs(job)))

    def test_every_cargo_call_uses_the_lockfile(self):
        call = re.compile(r'\bcargo\s+(?:\+\S+\s+)?(build|test|clippy|run|fetch|check|install|doc)\b[^\n;&|]*')
        for wf, name, job in jobs():
            for text in runs(job):
                for m in call.finditer(text):
                    self.assertRegex(m.group(0), r'--(locked|frozen)\b', f'{wf}/{name}: {m.group(0)}')

    def test_the_proton_patch_takes_no_fuzz_and_both_checks_use_one_commit(self):
        runtime = (REPO / '.github/workflows/runtime.yml').read_text()
        self.assertRegex(runtime, r'patch -p1 --fuzz=0 [^\n]*patches/proton/')
        base = re.search(r'PROTON_BASE=\$\{PROTON_BASE:-([0-9a-f]{40})\}',
                         (REPO / 'scripts/check-patches.sh').read_text()).group(1)
        defaults = re.findall(r"default: '([0-9a-f]+)'|INPUT_REF:-([0-9a-f]+)\}", runtime)
        self.assertEqual(len(defaults), 2)
        for ref in (a or b for a, b in defaults):
            self.assertTrue(base.startswith(ref), f'{ref} is not {base}')

    def test_ci_runs_for_every_script_and_packaging_change(self):
        triggers = load(REPO / '.github/workflows/rust.yml')[True]
        for event in ('push', 'pull_request'):
            for pattern in ('scripts/**', 'packaging/**', 'Cargo.lock', '.github/workflows/**'):
                self.assertIn(pattern, triggers[event]['paths'], event)


class PackagingTests(unittest.TestCase):
    def test_cargo_lock_is_committed(self):
        ignored = [l.strip() for l in (REPO / '.gitignore').read_text().splitlines()]
        self.assertNotIn('Cargo.lock', ignored)
        if (REPO / '.git').exists():
            tracked = subprocess.run(['git', '-C', str(REPO), 'ls-files', '--error-unmatch',
                                      'Cargo.lock'], capture_output=True)
            self.assertEqual(tracked.returncode, 0, 'Cargo.lock is not tracked')

    def test_the_aur_package_ships_the_window_it_has_a_desktop_entry_for(self):
        pkg = REPO / 'packaging/aur/ferestre-git'
        pkgbuild = (pkg / 'PKGBUILD').read_text()
        self.assertRegex(pkgbuild, r'install -Dm755 target/release/ferestre-gui +"\$pkgdir/usr/bin/ferestre-gui"')
        srcinfo = (pkg / '.SRCINFO').read_text()
        for dep in ('gtk4', 'libadwaita'):
            self.assertIn(f"'{dep}'", pkgbuild)
            self.assertIn(f'\tdepends = {dep}\n', srcinfo)
        desktop = (REPO / 'packaging/appimage/io.github.icex.ferestre.desktop').read_text()
        self.assertIn('TryExec=ferestre-gui', desktop)
        self.assertIn('cargo fetch --locked', pkgbuild)


if __name__ == '__main__':
    unittest.main()
