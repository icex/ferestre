#!/usr/bin/env python3
"""What a runtime build is built from, and whether main has already built it.

    runtime-inputs.py digest [--rev SHA] [--json]
    runtime-inputs.py find --digest D --tag-sha SHA --repo OWNER/NAME [--wait-minutes N]
    runtime-inputs.py manifest --out FILE --digest D --sdk IMAGE --dist FILE --built-proton SHA
    runtime-inputs.py check --zip FILE --api-digest sha256:HEX --manifest FILE --dist FILE
                            --digest D --run-id ID --head-sha SHA --repo OWNER/NAME
                            [--ref REF] [--expect-proton SHA]

`digest` is a SHA-256 over a canonical JSON of everything that decides what
`make dist` produces: the repository and commit of Proton, Wine, xgameruntime
and vkd3d-proton from sources.lock.json, and the git blob id of
scripts/build-runtime.sh (which holds every configure and make flag), and the
blob id of every patch under packaging/runtime-patches/ it applies. Proton's
own commit pins its SDK image and every other submodule. A lock edit that
leaves these alone (a dlssnr or xodus pin, a modified_paths list), or a change
to the workflow file, does not change the digest, so it does not rebuild the
runtime.

`find` is how a runtime-* tag avoids rebuilding what main already built. It
looks for a run of .github/workflows/runtime.yml in this repository that:

  - was a push to main (not a tag, not a pull request, not a manual run --
    a manual run may build any Proton ref somebody typed),
  - is at a commit whose own digest, recomputed here from that commit's
    files, is D,
  - is at a commit that is an ancestor of the tag (and the workflow checks
    that the tag is on main),
  - has a `build` job that succeeded,
  - uploaded an unexpired artefact named runtime-dist-D that has a digest.

The newest such run wins. If none has finished but one with the same digest is
still running, `find` waits for it (up to --wait-minutes). Otherwise the tag
builds the runtime itself. It writes found=, run_id=, artifact_id=,
artifact_digest= and head_sha= to $GITHUB_OUTPUT.

`manifest` writes the manifest.json that travels inside the artefact, from the
build's environment. `check` is what the package job runs on the downloaded
zip before using it: the zip's SHA-256 must be the digest GitHub recorded at
upload, and the manifest must name the same digest, run, commit and ref.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 'ferestre-runtime-inputs/1'
COMPONENTS = ('proton', 'wine', 'xgameruntime', 'vkd3d-proton')
BUILD_SCRIPT = 'scripts/build-runtime.sh'
PATCHES = 'packaging/runtime-patches'
WORKFLOW = '.github/workflows/runtime.yml'
LOCK = 'sources.lock.json'
SHA = re.compile(r'[0-9a-f]{40}')
DIGEST = re.compile(r'[0-9a-f]{64}')


def git(*args, cwd=ROOT):
    return subprocess.run(['git', '-C', str(cwd), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def inputs(lock_text, script_blob, patches=None):
    """The canonical inputs, from the lock's text, the build script's blob id
    and the patches' blob ids by path. No patches leaves the inputs as they
    were before there were any, so older builds keep their digest."""
    components = json.loads(lock_text)['components']
    picked = {}
    for name in COMPONENTS:
        pin = components[name]
        if not SHA.fullmatch(pin['commit']):
            raise ValueError(f'{name}: not a full commit')
        picked[name] = {'repository': pin['repository'], 'commit': pin['commit']}
    if not SHA.fullmatch(script_blob or ''):
        raise ValueError(f'{BUILD_SCRIPT}: no blob id')
    data = {'schema': SCHEMA, 'components': picked, 'build_script': script_blob}
    if patches:
        for path, blob in patches.items():
            if not SHA.fullmatch(blob):
                raise ValueError(f'{path}: no blob id')
        data['patches'] = dict(sorted(patches.items()))
    return data


def patches_at(rev=None, cwd=ROOT):
    """The `*.patch` files build-runtime.sh applies, by path, with blob ids."""
    if rev is None:
        root = Path(cwd)
        return {str(p.relative_to(root)): git('hash-object', str(p), cwd=cwd)
                for p in sorted((root / PATCHES).glob('*/*.patch'))}
    out = git('ls-tree', '-r', rev, '--', PATCHES, cwd=cwd)
    found = {}
    for line in out.splitlines():
        meta, path = line.split('\t', 1)
        if path.endswith('.patch') and path.count('/') == PATCHES.count('/') + 2:
            found[path] = meta.split()[2]
    return found


def digest_of(data):
    text = json.dumps(data, sort_keys=True, separators=(',', ':'))
    return hashlib.sha256(text.encode()).hexdigest()


def inputs_at(rev=None, cwd=ROOT):
    """Inputs at a commit, or in the working tree when rev is None. None when
    the commit has no build script (it predates this way of building)."""
    try:
        if rev is None:
            lock = (Path(cwd) / LOCK).read_text()
            blob = git('hash-object', BUILD_SCRIPT, cwd=cwd)
        else:
            lock = git('show', f'{rev}:{LOCK}', cwd=cwd)
            blob = git('rev-parse', f'{rev}:{BUILD_SCRIPT}', cwd=cwd)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return inputs(lock, blob, patches_at(rev, cwd))


def digest_at(rev=None, cwd=ROOT):
    data = inputs_at(rev, cwd)
    return digest_of(data) if data else None


# --- finding a main build to promote ----------------------------------------

def artifact_name(digest):
    return f'runtime-dist-{digest}'


def run_rejection(run, repo, digest, tag_sha, digest_at, is_ancestor):
    """Why a run cannot be a source for this digest, or None if it can (so
    far: its jobs and artefacts are checked separately)."""
    if run.get('path') != WORKFLOW:
        return f"workflow {run.get('path')}"
    if run.get('event') != 'push':
        return f"event {run.get('event')}"
    if run.get('head_branch') != 'main':
        return f"branch {run.get('head_branch')}"
    for key in ('repository', 'head_repository'):
        if (run.get(key) or {}).get('full_name') != repo:
            return f"{key} {(run.get(key) or {}).get('full_name')}"
    sha = run.get('head_sha') or ''
    if not SHA.fullmatch(sha):
        return 'no head commit'
    if not is_ancestor(sha, tag_sha):
        return f'{sha[:12]} is not an ancestor of the tag'
    if digest_at(sha) != digest:
        return f'{sha[:12]} has other runtime inputs'
    return None


def artifact_rejection(run, jobs, artifacts, digest):
    """Why a finished run's output cannot be promoted, or None."""
    build = [j for j in jobs if j.get('name') == 'build']
    if len(build) != 1:
        return 'no single build job'
    if build[0].get('conclusion') != 'success':
        return f"build {build[0].get('conclusion')}"
    named = [a for a in artifacts if a.get('name') == artifact_name(digest)]
    if len(named) != 1:
        return f'{len(named)} artefacts named {artifact_name(digest)}'
    art = named[0]
    if art.get('expired'):
        return 'artefact expired'
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', art.get('digest') or ''):
        return 'artefact has no digest'
    wr = art.get('workflow_run') or {}
    if wr.get('id') != run['id'] or wr.get('head_sha') != run['head_sha']:
        return 'artefact belongs to another run'
    return None


def select(runs, jobs_for, artifacts_for, *, repo, digest, tag_sha, digest_at, is_ancestor, log=print):
    """('promote', run, artefact) | ('wait', run, None) | ('build', None, None)."""
    waiting = None
    for run in sorted(runs, key=lambda r: (r.get('run_number', 0), r['id']), reverse=True):
        why = run_rejection(run, repo, digest, tag_sha, digest_at, is_ancestor)
        if why:
            log(f"  run {run['id']}: {why}")
            continue
        if run.get('status') != 'completed':
            log(f"  run {run['id']}: same inputs, still {run.get('status')}")
            waiting = waiting or run
            continue
        jobs, artifacts = jobs_for(run['id']), artifacts_for(run['id'])
        why = artifact_rejection(run, jobs, artifacts, digest)
        if why:
            log(f"  run {run['id']}: {why}")
            continue
        art = next(a for a in artifacts if a['name'] == artifact_name(digest))
        log(f"  run {run['id']}: built {digest[:12]} at {run['head_sha'][:12]}, artefact {art['id']}")
        return 'promote', run, art
    if waiting:
        return 'wait', waiting, None
    return 'build', None, None


def gh_json(path):
    out = subprocess.run(['gh', 'api', '-H', 'Accept: application/vnd.github+json', path],
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def cmd_find(args):
    if not DIGEST.fullmatch(args.digest) or not SHA.fullmatch(args.tag_sha):
        raise SystemExit('find: --digest is 64 hex digits and --tag-sha a full commit')
    repo = args.repo

    def is_ancestor(a, b):
        return subprocess.run(['git', '-C', str(ROOT), 'merge-base', '--is-ancestor', a, b]).returncode == 0

    def runs():
        data = gh_json(f'repos/{repo}/actions/workflows/runtime.yml/runs?branch=main&event=push&per_page=50')
        return data['workflow_runs']

    jobs_for = lambda rid: gh_json(f'repos/{repo}/actions/runs/{rid}/jobs?per_page=100')['jobs']
    artifacts_for = lambda rid: gh_json(
        f'repos/{repo}/actions/runs/{rid}/artifacts?per_page=100&name={artifact_name(args.digest)}')['artifacts']
    deadline = time.monotonic() + args.wait_minutes * 60
    while True:
        print(f'looking for a main build of {args.digest}')
        verdict, run, art = select(runs(), jobs_for, artifacts_for, repo=repo, digest=args.digest,
                                   tag_sha=args.tag_sha, digest_at=lambda s: digest_at(s),
                                   is_ancestor=is_ancestor)
        if verdict != 'wait' or time.monotonic() >= deadline:
            break
        print(f"waiting for run {run['id']} ({run.get('html_url', '')})", flush=True)
        time.sleep(args.poll_seconds)
    out = {'found': 'false'}
    if verdict == 'promote':
        out = {'found': 'true', 'run_id': str(run['id']), 'artifact_id': str(art['id']),
               'artifact_digest': art['digest'], 'head_sha': run['head_sha']}
        print(f"promote: run {run['id']}, artefact {art['id']} ({art['digest']})")
    elif verdict == 'wait':
        print(f"run {run['id']} has the same inputs but did not finish in {args.wait_minutes} min; building")
    else:
        print('no main build of these inputs; building')
    write_outputs(out)
    return 0


def write_outputs(values):
    text = ''.join(f'{k}={v}\n' for k, v in values.items())
    target = os.environ.get('GITHUB_OUTPUT')
    if target:
        with open(target, 'a') as f:
            f.write(text)
    sys.stdout.write(text)


# --- the manifest inside the artefact ------------------------------------------

def cmd_manifest(args):
    data = inputs_at(None)
    if data is None or digest_of(data) != args.digest:
        raise SystemExit('manifest: the checkout does not have the inputs being built')
    env = os.environ
    manifest = {
        'schema': 'ferestre-runtime-dist/1',
        'digest': args.digest,
        'inputs': data,
        'sdk_image': args.sdk,
        'head_sha': env['GITHUB_SHA'],
        'ref': env['GITHUB_REF'],
        'event': env['GITHUB_EVENT_NAME'],
        'run_id': int(env['GITHUB_RUN_ID']),
        'run_attempt': int(env.get('GITHUB_RUN_ATTEMPT', '1')),
        'repository': env['GITHUB_REPOSITORY'],
        'built_proton': args.built_proton,
        'dist_sha256': file_sha256(args.dist),
    }
    Path(args.out).write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    print(Path(args.out).read_text())
    return 0


def file_sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def check_manifest(manifest, *, digest, run_id, head_sha, ref, repo, dist_sha256, expect_proton=None):
    problems = []
    if manifest.get('schema') != 'ferestre-runtime-dist/1':
        problems.append('unknown manifest schema')
    if manifest.get('digest') != digest:
        problems.append(f"built {manifest.get('digest')}, need {digest}")
    if digest_of(manifest.get('inputs') or {}) != digest:
        problems.append('the recorded inputs do not hash to the digest')
    if manifest.get('run_id') != run_id:
        problems.append(f"built by run {manifest.get('run_id')}, not {run_id}")
    if manifest.get('head_sha') != head_sha:
        problems.append(f"built at {manifest.get('head_sha')}, not {head_sha}")
    if ref is not None and manifest.get('ref') != ref:
        problems.append(f"built for {manifest.get('ref')}, not {ref}")
    if ref == 'refs/heads/main' and manifest.get('event') != 'push':
        problems.append(f"built by a {manifest.get('event')} event")
    if manifest.get('repository') != repo:
        problems.append(f"built in {manifest.get('repository')}")
    if manifest.get('dist_sha256') != dist_sha256:
        problems.append('dist.tar.zst is not the file the manifest names')
    if expect_proton is not None:
        pinned = ((manifest.get('inputs') or {}).get('components') or {}).get('proton', {}).get('commit')
        if not (manifest.get('built_proton') == expect_proton == pinned):
            problems.append(f"built Proton {manifest.get('built_proton')}, not the pinned {expect_proton}")
    return problems


def cmd_check(args):
    algo, _, want = args.api_digest.partition(':')
    if algo != 'sha256' or not DIGEST.fullmatch(want):
        raise SystemExit(f'check: GitHub recorded no usable digest ({args.api_digest!r})')
    got = file_sha256(args.zip)
    if got != want:
        raise SystemExit(f'check: the downloaded artefact is {got}, GitHub recorded {want}')
    manifest = json.loads(Path(args.manifest).read_text())
    problems = check_manifest(manifest, digest=args.digest, run_id=int(args.run_id),
                              head_sha=args.head_sha, ref=args.ref, repo=args.repo,
                              dist_sha256=file_sha256(args.dist),
                              expect_proton=args.expect_proton or None)
    if problems:
        raise SystemExit('check: ' + '; '.join(problems))
    print(f"artefact {got[:12]} from run {args.run_id} at {args.head_sha[:12]}: inputs {args.digest[:12]}")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    sub = p.add_subparsers(dest='cmd', required=True)
    d = sub.add_parser('digest')
    d.add_argument('--rev', help='a commit (default: the working tree)')
    d.add_argument('--json', action='store_true', help='print the inputs as well')
    f = sub.add_parser('find')
    f.add_argument('--digest', required=True)
    f.add_argument('--tag-sha', required=True)
    f.add_argument('--repo', required=True)
    f.add_argument('--wait-minutes', type=int, default=0)
    f.add_argument('--poll-seconds', type=int, default=60)
    m = sub.add_parser('manifest')
    m.add_argument('--out', required=True)
    m.add_argument('--digest', required=True)
    m.add_argument('--sdk', required=True)
    m.add_argument('--dist', required=True, help='dist.tar.zst, whose SHA-256 the manifest records')
    m.add_argument('--built-proton', required=True, help='the Proton commit that was built')
    c = sub.add_parser('check')
    for name in ('--zip', '--api-digest', '--manifest', '--dist', '--digest', '--run-id',
                 '--head-sha', '--repo'):
        c.add_argument(name, required=True)
    c.add_argument('--ref', help='the ref the build must have been for')
    c.add_argument('--expect-proton', help='the Proton commit the build must have built (the pin)')
    args = p.parse_args()
    if args.cmd == 'digest':
        data = inputs_at(args.rev)
        if data is None:
            raise SystemExit(f'{args.rev or "the working tree"} has no {BUILD_SCRIPT}')
        if args.json:
            print(json.dumps(data, indent=2, sort_keys=True))
        print(digest_of(data))
        return 0
    return {'find': cmd_find, 'manifest': cmd_manifest, 'check': cmd_check}[args.cmd](args)


if __name__ == '__main__':
    sys.exit(main())
