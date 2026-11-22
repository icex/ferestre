#!/usr/bin/env python3
"""Run the GUI tests the way the CI `gui` job does, one shard of them.

    tests/run_gui_ci.py --shard 2/4 [--binary target/debug/ferestre-gui]
    tests/run_gui_ci.py --list --shards 4      # print the split, run nothing
    tests/run_gui_ci.py --only smoke,move      # run named tests, any shard

Every test in tests/gui_ci.json gets its own Xvfb (a free display picked by
the server itself through -displayfd, so two tests, two shards or a person's
own X server never collide) and its own session bus (dbus-run-session), with
the screen size the manifest names. Tests run one after another inside a
shard; the shards run in parallel as separate CI jobs.

The split is by the manifest's weights (seconds measured in CI): the heaviest
test goes to the lightest shard, which keeps the shards within a few seconds
of each other. gui_smoke.py alone is about 150 s, which is the floor for any
number of shards.

Every test runs even after one fails, so a single run shows every failure.
Each test's own output and its Xvfb log are printed in a collapsible group,
and the timings go to $GITHUB_STEP_SUMMARY when it is set. Exits 1 if any test
failed.

Needs Xvfb, dbus-run-session, at-spi2-core and python3-gi (the CI image, or
the Ubuntu 24.04 container in docs/TESTING.md). On a desktop, pass
--dbus-config with a session config that activates nothing but the
accessibility bus, so no test can start the real keyring daemon.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]
MANIFEST = REPO / 'tests/gui_ci.json'


def load(path=MANIFEST):
    tests = json.loads(Path(path).read_text())['tests']
    names = [t['name'] for t in tests]
    if len(names) != len(set(names)):
        raise ValueError('test names in the GUI manifest must be unique')
    for t in tests:
        if set(t) - {'name', 'script', 'args', 'screen', 'weight'}:
            raise ValueError(f"{t['name']}: unknown keys")
        w, h = (int(x) for x in t['screen'].split('x'))
        if not (w > 0 and h > 0 and int(t['weight']) > 0):
            raise ValueError(f"{t['name']}: bad screen or weight")
    return tests


def split(tests, shards):
    """Longest first onto the lightest shard. Deterministic: ties go by name
    and then to the lower shard number."""
    loads = [0] * shards
    plan = [[] for _ in range(shards)]
    for t in sorted(tests, key=lambda t: (-t['weight'], t['name'])):
        k = min(range(shards), key=lambda i: (loads[i], i))
        plan[k].append(t)
        loads[k] += t['weight']
    return plan


def start_xvfb(screen, log):
    read, write = os.pipe()
    proc = subprocess.Popen(
        ['Xvfb', '-displayfd', str(write), '-screen', '0', f'{screen}x24',
         '-noreset', '-nolisten', 'tcp'],
        pass_fds=(write,), stdout=log, stderr=log)
    os.close(write)
    with os.fdopen(read) as f:
        number = f.readline().strip()
    if not number:
        proc.kill()
        raise RuntimeError('Xvfb did not report a display')
    return proc, f':{number}'


def run_one(test, binary, dbus_config, timeout, work):
    log_path = work / f"{test['name']}.xvfb.log"
    out_path = work / f"{test['name']}.log"
    started = time.monotonic()
    with open(log_path, 'wb') as xlog, open(out_path, 'wb') as out:
        xvfb, display = start_xvfb(test['screen'], xlog)
        try:
            env = dict(os.environ, DISPLAY=display)
            env.pop('WAYLAND_DISPLAY', None)
            env.setdefault('GDK_BACKEND', 'x11')
            env.setdefault('GSK_RENDERER', 'cairo')
            dbus = ['dbus-run-session']
            if dbus_config:
                dbus += ['--config-file', dbus_config]
            cmd = dbus + ['--', sys.executable, test['script'],
                          '--binary', str(binary), *test.get('args', [])]
            # A session of its own, so a test that hangs is taken down with
            # everything it started -- the window, the CLI, the bus -- and
            # nothing is left holding the display or the runner.
            proc = subprocess.Popen(cmd, cwd=REPO, env=env, stdout=out,
                                    stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = proc.wait(timeout)
            except subprocess.TimeoutExpired:
                code = 'timeout'
            finally:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                proc.wait()
        finally:
            xvfb.terminate()
            try:
                xvfb.wait(10)
            except subprocess.TimeoutExpired:
                xvfb.kill()
    return code, time.monotonic() - started, out_path, log_path


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    p.add_argument('--shard', help='k/n: run the k-th of n shards (1-based)')
    p.add_argument('--only', help='comma-separated test names from the manifest')
    p.add_argument('--list', action='store_true', help='print the split and exit')
    p.add_argument('--shards', type=int, default=4, help='shard count for --list')
    p.add_argument('--binary', default='target/debug/ferestre-gui',
                   help='ferestre-gui; the ferestre CLI must sit beside it')
    p.add_argument('--manifest', default=str(MANIFEST))
    p.add_argument('--dbus-config', help='session bus config for dbus-run-session')
    p.add_argument('--timeout', type=int, default=600, help='seconds per test')
    args = p.parse_args()

    tests = load(args.manifest)
    if args.list:
        for k, part in enumerate(split(tests, args.shards), 1):
            names = ', '.join(t['name'] for t in part)
            print(f"shard {k}/{args.shards}: {sum(t['weight'] for t in part):4d} s  {names}")
        return 0
    if args.only:
        wanted = args.only.split(',')
        unknown = set(wanted) - {t['name'] for t in tests}
        if unknown:
            p.error('not in the manifest: ' + ', '.join(sorted(unknown)))
        chosen = [t for t in tests if t['name'] in wanted]
        label = 'selected'
    elif args.shard:
        k, n = (int(x) for x in args.shard.split('/'))
        if not 1 <= k <= n:
            p.error('--shard is k/n with 1 <= k <= n')
        chosen = split(tests, n)[k - 1]
        label = f'shard {k}/{n}'
    else:
        chosen = tests
        label = 'all'

    binary = (REPO / args.binary).resolve()
    for name in ('ferestre-gui', 'ferestre'):
        if not binary.with_name(name).is_file():
            print(f'{binary.with_name(name)} is not there; '
                  'cargo build --locked -p ferestre-gui -p ferestre-cli', file=sys.stderr)
            return 2
    for tool in ('Xvfb', 'dbus-run-session'):
        if not shutil.which(tool):
            print(f'{tool} is not installed', file=sys.stderr)
            return 2

    gha = 'GITHUB_ACTIONS' in os.environ
    results = []
    with tempfile.TemporaryDirectory(prefix='ferestre-gui-ci-') as tmp:
        work = Path(tmp)
        print(f"{label}: {len(chosen)} tests, about {sum(t['weight'] for t in chosen)} s",
              flush=True)
        for t in chosen:
            code, secs, out, xlog = run_one(t, binary, args.dbus_config, args.timeout, work)
            ok = code == 0
            results.append((t, ok, code, secs))
            head = f"{'ok  ' if ok else 'FAIL'} {t['name']} ({t['script']} {' '.join(t.get('args', []))}".rstrip()
            head += f", {t['screen']}) {secs:.0f} s"
            print(f'::group::{head}' if gha else f'--- {head}', flush=True)
            sys.stdout.write(out.read_text(errors='replace'))
            if not ok:
                print(f'--- exit {code}; Xvfb log:')
                sys.stdout.write(xlog.read_text(errors='replace'))
            if gha:
                print('::endgroup::')
            if not ok and gha:
                print(f"::error title=GUI test {t['name']} failed::{t['script']} exited {code}")
            sys.stdout.flush()

    total = sum(r[3] for r in results)
    lines = [f'### GUI tests, {label}', '', '| Test | Result | Seconds | Weight |', '|---|---|---|---|']
    for t, ok, code, secs in results:
        lines.append(f"| {t['name']} | {'ok' if ok else f'failed ({code})'} | {secs:.0f} | {t['weight']} |")
    lines.append(f'| total | | {total:.0f} | {sum(t["weight"] for t in chosen)} |')
    print('\n'.join(lines))
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as f:
            f.write('\n'.join(lines) + '\n')
    failed = [t['name'] for t, ok, *_ in results if not ok]
    if failed:
        print('failed: ' + ', '.join(failed), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
