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
# A workflow_dispatch run started on a tag ref has ref_type 'tag' as well, so
# signing and publishing ask for the push event too.
TAG_PUSH = "github.event_name == 'push' && github.ref_type == 'tag'"
NOT_TAG_PUSH = "github.event_name != 'push' || github.ref_type != 'tag'"


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

    def test_only_a_tag_push_signs_or_publishes(self):
        for path in (REPO / '.github/workflows/release.yml',
                     REPO / '.github/workflows/runtime.yml',
                     REPO / '.github/workflows/dlssnr.yml'):
            text = path.read_text()
            conditions = [c for c in re.findall(r'^\s*if: (.*)$', text, re.M)
                          if 'ref_type' in c]
            self.assertIn(TAG_PUSH, conditions, path.name)
            for cond in conditions:
                self.assertIn(cond, (TAG_PUSH, NOT_TAG_PUSH), path.name)

    def test_a_tagged_runtime_builds_the_pinned_proton(self):
        job = load(REPO / '.github/workflows/runtime.yml')['jobs']['plan']
        step = next(s for s in job['steps'] if s.get('id') == 'ref')
        self.assertIn('PINNED_REF=$(python3 scripts/source-pins.py get proton commit)', step['run'])
        run = step['run']
        self.assertIn('ref=$PINNED_REF', run)
        self.assertIn('[ "$GITHUB_EVENT_NAME" = workflow_dispatch ]', run)
        self.assertIn('[ "$GITHUB_REF_TYPE" != tag ]', run)
        self.assertNotIn('${{', run)

    def test_the_signing_key_reaches_only_the_publishing_jobs(self):
        for path in WORKFLOWS:
            workflow = load(path)
            for name, job in workflow.get('jobs', {}).items():
                if 'secrets.MINISIGN' in yaml.safe_dump(job):
                    self.assertTrue(can_write(job, workflow), f'{path.name}/{name}')
                    # After the job that made the artefacts: build, or for the
                    # runtime, package (which checks a promoted main build).
                    self.assertIn(job.get('needs'), ('build', ['plan', 'package']),
                                  f'{path.name}/{name}')
                    # Environment secrets, so the environment's tag rule and
                    # required reviewer gate every job that can read them.
                    self.assertEqual(job.get('environment'), 'release', f'{path.name}/{name}')
                    self.assertEqual(job.get('if'), TAG_PUSH, f'{path.name}/{name}')
                    for step in job.get('steps', []):
                        if 'secrets.MINISIGN' in yaml.safe_dump(step):
                            self.assertEqual(step.get('if'), TAG_PUSH,
                                             f'{path.name}/{name}/{step.get("name")}')

    def test_releases_are_signed_before_they_are_published(self):
        for wf in ('release.yml', 'runtime.yml', 'dlssnr.yml'):
            workflow = load(REPO / '.github/workflows' / wf)
            publishing = [(n, j) for n, j in workflow['jobs'].items()
                          if any('gh release create' in t for t in runs(j))]
            self.assertEqual([n for n, _ in publishing], ['publish'], wf)
            job = publishing[0][1]
            self.assertEqual(job.get('if'), TAG_PUSH, wf)
            texts = list(runs(job))
            sign = next(i for i, t in enumerate(texts) if 'packaging/sign-release.sh out' in t)
            publish = next(i for i, t in enumerate(texts) if 'gh release create' in t)
            self.assertLess(sign, publish, wf)
            self.assertIn('out/*', texts[publish], wf)
            self.assertFalse(can_write(workflow['jobs']['build'], workflow), wf)

    def test_signatures_carry_the_tags_version_and_manual_runs_say_they_are_unsigned(self):
        for wf, prefix in (('release.yml', 'v'), ('runtime.yml', 'runtime-'),
                           ('dlssnr.yml', 'dlssnr-')):
            workflow = load(REPO / '.github/workflows' / wf)
            sign = next(t for t in runs(workflow['jobs']['publish'])
                        if 'packaging/sign-release.sh out' in t)
            self.assertIn(f'tag_version=${{GITHUB_REF_NAME#{prefix}}}', sign, wf)
            self.assertIn('packaging/sign-release.sh out "$tag_version"', sign, wf)
            # The runtime's tarball is made in `package`, from this run's
            # build or a promoted main build.
            maker = 'package' if wf == 'runtime.yml' else 'build'
            unsigned = [s for s in workflow['jobs'][maker]['steps']
                        if s.get('name') == 'Unsigned build']
            self.assertEqual(len(unsigned), 1, wf)
            self.assertEqual(unsigned[0].get('if'), NOT_TAG_PUSH, wf)
            self.assertIn('UNSIGNED', unsigned[0]['run'], wf)
            self.assertIn('GITHUB_STEP_SUMMARY', unsigned[0]['run'], wf)

    def test_the_dlssnr_package_is_built_from_the_lock_and_never_becomes_latest(self):
        workflow = load(REPO / '.github/workflows/dlssnr.yml')
        text = '\n'.join(runs(workflow['jobs']['build']))
        self.assertIn('scripts/build-dlssnr.sh out', text)
        self.assertIn('The tag must be on main',
                      [s.get('name') for s in workflow['jobs']['build']['steps']])
        self.assertNotIn('${{', text)
        publish = next(t for t in runs(workflow['jobs']['publish']) if 'gh release create' in t)
        # The README's "latest release" link must keep pointing at the AppImage.
        self.assertIn('--latest=false', publish)

    def test_the_dlssnr_build_sums_every_file_it_writes(self):
        text = (REPO / 'scripts/build-dlssnr.sh').read_text()
        sums = [i for i, l in enumerate(text.splitlines()) if 'SHA256SUMS' in l and 'sha256sum' in l]
        self.assertEqual(len(sums), 1)
        line = text.splitlines()[sums[0]]
        self.assertIn('sha256sum -- *', line)  # every file in the output dir, SOURCES.txt included
        # Nothing is written into the output dir after the sums.
        self.assertNotRegex('\n'.join(text.splitlines()[sums[0] + 1:]), r'>\s*"?\$out')

    def test_the_msrv_job_builds_with_the_declared_rust_version(self):
        declared = re.search(r'^rust-version = "([0-9.]+)"$',
                             (REPO / 'Cargo.toml').read_text(), re.M).group(1)
        job = load(REPO / '.github/workflows/rust.yml')['jobs']['msrv']
        toolchains = [s['with']['toolchain'] for s in job['steps']
                      if str(s.get('uses', '')).startswith('dtolnay/rust-toolchain@')]
        self.assertEqual(toolchains, [declared])
        self.assertTrue(any(f'cargo +{declared} test --locked --workspace' in t
                            for t in runs(job)))
        # A pull request type-checks every target, tests included.
        self.assertTrue(any(f'cargo +{declared} check --locked --workspace --all-targets' in t
                            for t in runs(job)))

    def test_releases_never_build_in_the_persistent_ci_directory(self):
        # Self-hosted CI jobs build in a directory that outlives the job and
        # that pull requests write to. A signed release must keep building in
        # a clean checkout from main's sources.
        for wf in ('release.yml', 'runtime.yml'):
            text = (REPO / '.github/workflows' / wf).read_text()
            for marker in ('ci-target-dir', 'ferestre-ci', 'CARGO_TARGET_DIR'):
                self.assertNotIn(marker, text, wf)
            # A runner shares its work directory between workflows, so the
            # `target` symlink a CI job left there is still in the checkout
            # when a release runs on that runner. checkout's clean step (the
            # default) removes it; a release must never turn that off.
            for _, job in load(REPO / '.github/workflows' / wf)['jobs'].items():
                for step in job.get('steps', []):
                    if str(step.get('uses', '')).startswith('actions/checkout@'):
                        self.assertNotIn('clean', step.get('with', {}), wf)

    def test_ci_uses_the_persistent_directory_or_the_cache_never_both(self):
        workflow = load(REPO / '.github/workflows/rust.yml')
        for name, job in workflow['jobs'].items():
            steps = job['steps']
            persistent = [s for s in steps if 'ci-target-dir.sh' in s.get('run', '')]
            cached = [s for s in steps if str(s.get('uses', '')).startswith('Swatinem/rust-cache@')]
            for step in cached:
                self.assertEqual(step['with'].get('save-if'),
                                 "${{ github.ref == 'refs/heads/main' }}", name)
            if name == 'test':
                # The one job that may run self-hosted (FERESTRE_RUNNER_LABELS).
                self.assertEqual(len(persistent), 1, name)
                self.assertEqual(len(cached), 1, name)
                self.assertEqual(persistent[0].get('if'), "runner.environment == 'self-hosted'", name)
                self.assertEqual(cached[0].get('if'), "runner.environment != 'self-hosted'", name)
            else:
                self.assertEqual(job['runs-on'], 'ubuntu-24.04', name)
                self.assertEqual(persistent, [], name)

    def test_only_the_test_job_takes_a_self_hosted_slot(self):
        for path in WORKFLOWS:
            for name, job in load(path)['jobs'].items():
                if path.name == 'rust.yml' and name == 'test':
                    self.assertIn('vars.FERESTRE_RUNNER_LABELS', job['runs-on'])
                    continue
                self.assertEqual(job['runs-on'], 'ubuntu-24.04', f'{path.name}/{name}')

    def test_no_ci_job_can_cancel_or_write(self):
        # A pull request run gets nothing but a read-only token.
        workflow = load(REPO / '.github/workflows/rust.yml')
        for name, job in workflow['jobs'].items():
            self.assertFalse(can_write(job, workflow), name)

    def test_the_gui_shards_cover_every_gui_test(self):
        workflow = load(REPO / '.github/workflows/rust.yml')
        gui = workflow['jobs']['gui']
        self.assertEqual(gui['strategy']['matrix']['shard'], [1, 2, 3, 4])
        self.assertIs(gui['strategy']['fail-fast'], False)
        self.assertEqual(gui['needs'], 'gui-build')
        self.assertTrue(any('tests/run_gui_ci.py --shard "$SHARD/4"' in t for t in runs(gui)))
        import json
        manifest = json.loads((REPO / 'tests/gui_ci.json').read_text())['tests']
        listed = {t['script'] for t in manifest}
        on_disk = {f'tests/{p.name}' for p in (REPO / 'tests').glob('gui_*.py')}
        self.assertEqual(listed, on_disk)
        # Nothing in the gui jobs still starts a test by hand.
        for name in ('gui-build', 'gui'):
            for text in runs(workflow['jobs'][name]):
                self.assertNotIn('xvfb-run', text, name)

    def test_release_trains_are_never_cancelled(self):
        for wf in ('runtime.yml', 'release.yml', 'dlssnr.yml'):
            concurrency = load(REPO / '.github/workflows' / wf)['concurrency']
            self.assertIs(concurrency['cancel-in-progress'], False, wf)
            self.assertIn('github.ref', concurrency['group'], wf)

    def test_only_main_fills_the_runtime_cache_and_makes_promotable_dists(self):
        job = load(REPO / '.github/workflows/runtime.yml')['jobs']['build']
        main_push = "github.event_name == 'push' && github.ref == 'refs/heads/main'"
        # A fixed path: the cache version includes it, and ccache only hits a
        # build at the same path.
        self.assertEqual(job['env']['CCACHE_DIR'], '/home/runner/.ccache')
        saves = [s for s in job['steps'] if str(s.get('uses', '')).startswith('actions/cache/save@')]
        self.assertEqual(len(saves), 1)
        self.assertEqual(saves[0]['if'],
                         f"always() && {main_push} && steps.ccache.outputs.cache-hit != 'true'")
        restore = next(s for s in job['steps'] if str(s.get('uses', '')).startswith('actions/cache/restore@'))
        self.assertEqual(restore['with']['restore-keys'].split(), ['runtime-ccache-v3-'])
        for step in job['steps']:
            self.assertFalse(str(step.get('uses', '')).startswith('actions/cache@'),
                             'a combined restore-and-save cache step would save from any ref')
        upload = next(s for s in job['steps'] if s.get('id') == 'upload')
        self.assertEqual(upload['with']['name'],
                         f"${{{{ ({main_push}) && format('runtime-dist-{{0}}', needs.plan.outputs.digest)"
                         " || format('runtime-dist-run-{0}', github.run_id) }}")
        self.assertIn('scripts/build-runtime.sh', '\n'.join(runs(job)))

    def test_a_tag_promotes_only_a_checked_main_build(self):
        workflow = load(REPO / '.github/workflows/runtime.yml')
        plan = workflow['jobs']['plan']
        find = next(s for s in plan['steps'] if s.get('id') == 'find')
        # Another Proton ref always builds.
        self.assertEqual(find['if'], "steps.ref.outputs.pinned == 'true'")
        self.assertIn('runtime-inputs.py find', find['run'])
        self.assertEqual(plan['permissions'], {'contents': 'read', 'actions': 'read'})
        self.assertIn('The tag must be on main', [s.get('name') for s in plan['steps']])
        self.assertEqual(workflow['jobs']['build']['if'], "needs.plan.outputs.build == 'true'")
        package = workflow['jobs']['package']
        self.assertFalse(can_write(package, workflow))
        text = '\n'.join(runs(package))
        # GitHub's recorded digest, the zip and the manifest are all checked
        # before anything is unpacked from the dist.
        self.assertIn('recorded=$(gh api "repos/$REPO/actions/artifacts/$artifact" --jq .digest)', text)
        self.assertIn('runtime-inputs.py check --zip dl/dist.zip --api-digest "$recorded"', text)
        self.assertIn('ref=refs/heads/main', text)
        self.assertLess(text.index('runtime-inputs.py check'), text.index('tar -x -C tree'))
        self.assertEqual(workflow['jobs']['publish']['needs'], ['plan', 'package'])

    def test_only_pull_request_runs_cancel_one_another(self):
        for wf in ('rust.yml', 'compatibility-matrix.yml'):
            concurrency = load(REPO / '.github/workflows' / wf)['concurrency']
            self.assertEqual(concurrency['cancel-in-progress'],
                             "${{ github.event_name == 'pull_request' }}", wf)
            # Anything but a pull request is a group of one.
            self.assertIn("github.event_name == 'pull_request' &&", concurrency['group'], wf)
            self.assertTrue(concurrency['group'].endswith('|| github.run_id }}'), wf)

    def test_the_test_job_always_runs_and_formatting_is_checked(self):
        workflow = load(REPO / '.github/workflows/rust.yml')
        self.assertNotIn('if', workflow['jobs']['test'])
        self.assertTrue(any('cargo fmt --all --check' in t
                            for t in runs(workflow['jobs']['preflight'])))

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

    def test_runtime_and_source_validation_read_the_same_locked_fork(self):
        build = (REPO / 'scripts/build-runtime.sh').read_text()
        self.assertIn('pins get proton commit', build)
        self.assertIn('pins get proton repository', build)
        self.assertIn('pins verify proton --directory "$work/proton"', build)
        for name in ('wine', 'xgameruntime', 'vkd3d-proton'):
            self.assertIn(f'pins verify {name} --directory', build)
        self.assertIn('../configure.sh --enable-ccache --build-name=ferestre', build)
        self.assertNotIn('patch -p1', build)
        self.assertNotIn('patch -p1', (REPO / '.github/workflows/runtime.yml').read_text())
        self.assertIn('source-pins.py', (REPO / 'scripts/check-sources.sh').read_text())

    def test_ci_runs_for_every_script_and_packaging_change(self):
        triggers = load(REPO / '.github/workflows/rust.yml')[True]
        for event in ('push', 'pull_request'):
            for pattern in ('scripts/**', 'packaging/**', 'Cargo.lock', '.github/workflows/**',
                            'tests/*.sh', 'tests/license_token_fixture.py',
                            'tests/test_no_nvidia_urls.py', 'tests/test_build_dlssnr.py'):
                self.assertIn(pattern, triggers[event]['paths'], event)

    def test_the_nvidia_url_guard_runs_on_every_change(self):
        # rust.yml is path-filtered, and a docs- or README-only change is the
        # likeliest place for such a URL, so the guard has an unfiltered home.
        workflow = load(REPO / '.github/workflows/guards.yml')
        triggers = workflow[True]
        for event in ('push', 'pull_request'):
            self.assertIn(event, triggers)
            self.assertFalse(triggers[event], f'{event} must not be filtered by path or branch')
        job = workflow['jobs']['no-nvidia-urls']
        self.assertNotIn('if', job)
        self.assertIn('python3 tests/test_no_nvidia_urls.py', list(runs(job)))
        for step in job['steps']:
            self.assertNotIn('if', step, step.get('name'))

    def test_namespace_tests_cannot_skip_in_ci(self):
        # stop.rs and the image-view tests skip where unprivileged user
        # namespaces are blocked, which a stock Ubuntu runner does until the
        # sysctl is lifted. Every job that runs them lifts it first and makes
        # a skip a failure.
        workflow = load(REPO / '.github/workflows/rust.yml')
        for name in ('test', 'msrv'):
            steps = workflow['jobs'][name]['steps']
            lift = next(i for i, s in enumerate(steps)
                        if s.get('name') == 'Allow unprivileged user namespaces')
            for i, step in enumerate(steps):
                if re.search(r'\bcargo\s+(\+\S+\s+)?test\b', step.get('run', '')):
                    self.assertLess(lift, i, f'{name}/{step.get("name")}')
                    self.assertEqual(step.get('env', {}).get('FERESTRE_REQUIRE_USERNS'), '1',
                                     f'{name}/{step.get("name")}')

    def test_the_pinned_client_fork_tests_run_in_ci(self):
        job = load(REPO / '.github/workflows/rust.yml')['jobs']['sources']
        text = '\n'.join(runs(job))
        for crate in ('-p xodus-service', '-p msixvc', '-p xodus-cli',
                      '--features key-chain-file --test key_chain_file'):
            self.assertIn(crate, text)


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
