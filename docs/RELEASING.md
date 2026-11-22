# Releasing

Three release trains, all driven by tags and nothing else:

| Tag | Workflow | Publishes |
|---|---|---|
| `v<version>`, e.g. `v0.1.14` | `.github/workflows/release.yml` | the launcher: `ferestre-<version>-x86_64.tar.zst`, `ferestre-<version>-x86_64.AppImage`, `SHA256SUMS` |
| `runtime-<version>`, e.g. `runtime-11.0.20260907` | `.github/workflows/runtime.yml` | the patched Proton: `ferestre-runtime-<version>.tar.xz`, `SHA256SUMS` |
| `dlssnr-<version>`, e.g. `dlssnr-0.0.9` | `.github/workflows/dlssnr.yml` | the DLSSNR-AMD Linux package: `ferestre-dlssnr-<version>-x86_64.tar.gz`, `ferestre-dlssnr-<version>-i686.tar.gz`, `SOURCES.txt`, `SHA256SUMS`; never marked latest |

Every published file has a minisign signature beside it, `<file>.minisig`. The
launcher checks a runtime's signature against the public key built into it
(`RELEASE_PUBLIC_KEY` in `crates/ferestre-core/src/signing.rs`) and refuses a
runtime that is unsigned or signed by any other key.

## How signing is wired

- Every job that compiles or packages runs with a read-only token and never
  sees the key: `build` in the DLSSNR workflow; `test`, `launcher`, `client`
  and `build` in the launcher's; `plan`, `build` and `package` in the
  runtime's. `publish` runs only for a tag push
  (`github.event_name == 'push' && github.ref_type == 'tag'`), after the job
  that made the artefacts succeeded (`build`, or the runtime's `package`). It
  is the only job with `contents: write` and the only job that names the
  `release` environment, so it is the only job that can read the signing
  secrets. It runs no build and no third-party action apart from checkout and
  download-artifact. All actions are pinned to a commit.
- All of these jobs run on GitHub-hosted `ubuntu-24.04` runners, never on the
  self-hosted host (see "Runner policy").
- The `Sign` step has the same condition, and
  `packaging/sign-release.sh` checks that every file matches `SHA256SUMS`
  before it signs anything.
- Each signature's trusted comment is exactly
  `file:<name> version:<version>`, where `<version>` is the tag without its
  prefix (`0.1.14` for `v0.1.14`, `11.0.20260907` for
  `runtime-11.0.20260907`). The trusted comment is signed too. The launcher
  accepts a runtime's signature only when its `file:` field names the file it
  downloaded and its `version:` field names the release it took the file
  from, so a valid signature cannot be moved onto a different file or
  replayed under another release. Only letters, digits, `.`, `_` and `-` are
  accepted in a version, starting with a letter or digit; the script refuses
  anything else, because the launcher would.
- Before signing, the script checks that the release's own key is the one the
  launcher trusts. For the launcher, it checks that the `ferestre` binary in
  the tarball contains the signing key's public half. For the runtime, it
  checks `signing.rs` at the tagged commit. If the key does not match, or
  `signing.rs` still has the placeholder, the release stops at `Sign` and
  nothing is published.
- **Manual runs (`workflow_dispatch`) are never signed**, even when started
  on a tag ref: the condition asks for a push event, not just a tag. They
  build the same artefacts and attach them to the run so you can check that a
  release would build. Only a manual run from a branch can build another
  Proton commit (`proton_ref`); one started on a tag builds the pinned commit
  or stops. Their summary says they are **UNSIGNED**, and `ferestre
  install-runtime` refuses such a runtime.

## One-time setup

### 1. Make the key pair

Do this on a machine you trust, not in CI:

```sh
minisign -G -p ferestre-release.pub -s ferestre-release.key
```

minisign asks for a password, and you should set one. `-W` makes a key with
no password; only use it if you understand that whoever gets the file can
sign with it.

- `ferestre-release.key` is the secret key. Keep an offline backup, for
  example on an encrypted USB stick or in a password manager. Never commit it.
- `ferestre-release.pub` is the public key. Its second line, the one that
  starts with `RW`, is the part the launcher needs.

### 2. Build the public key into the launcher

In `crates/ferestre-core/src/signing.rs`, replace only the value of
`RELEASE_PUBLIC_KEY`:

```rust
pub const RELEASE_PUBLIC_KEY: &str = "RW...";   // second line of ferestre-release.pub
```

Leave `RELEASE_PUBLIC_KEY_PLACEHOLDER` as it is. Commit the change and merge
it to `main` before you cut either kind of tag.

### 3. Create the `release` environment and store the secret there

The secrets go in an environment, not in the repository's secrets. Repository
secrets are available to every workflow and every job that names them.
Environment secrets are available only to jobs that name the environment, and
only once the environment's rules allow it.

Under **Settings > Environments > New environment**, create `release`:

- **Deployment branches and tags:** choose *Selected branches and tags* and
  add three tag rules, `v*`, `runtime-*` and `dlssnr-*`. Add no branch rules. A run from a
  branch, including a manual run, then cannot reach the key, even if someone
  changes a workflow to try.
- **Required reviewers:** add yourself, or whoever approves releases. Every
  publish then waits for an approval in the Actions tab. If you are the only
  maintainer, leave *Prevent self-review* off, or you cannot approve your own
  release.
- **Environment secrets:**
  - `MINISIGN_SECRET_KEY`: the whole contents of `ferestre-release.key`,
    both lines.
  - `MINISIGN_PASSWORD`: the key's password. Leave it unset for a key made
    with `-W`.

The same can be done with the GitHub CLI. The environment and its rules still
have to be set up in the web UI:

```sh
gh secret set MINISIGN_SECRET_KEY --env release < ferestre-release.key
gh secret set MINISIGN_PASSWORD   --env release        # prompts for the value
```

If repository-level secrets with these names exist from an earlier setup,
delete them (`gh secret delete MINISIGN_SECRET_KEY`, and the same for
`MINISIGN_PASSWORD`), so the key lives in one place only.

Create the environment **before** the first tag is pushed. If a job names an
environment that does not exist, GitHub creates it with no rules.

You can also protect the tags themselves, under **Settings > Rules >
Rulesets**: a tag ruleset for `v*`, `runtime-*` and `dlssnr-*` that limits who can create,
move or delete them. The environment controls who can use the key; the
ruleset controls who can start a release at all.

**Checked on 2026-11-22, the repository does not yet fully match this
section.** The `release` environment has the three tag rules but no required
reviewer, so every `publish` runs without an approval. There is no tag
ruleset, and `main` has no branch protection. The "tag must be on main" check
lives in the tagged commit's own workflow, so it holds only for tags of
commits whose workflow has it; a ruleset that limits who may create release
tags is what makes it binding. Add the reviewer and the ruleset.

## Runner policy

The self-hosted runners share one host with four build slots, and every
runner user there also runs same-repository pull request jobs, which can write
anything in that user's home: `~/.cargo/bin`, `~/.rustup`, the rootless
Docker image store. A job that signs, or builds what is signed, must not
start from that state. So:

- **Hosted `ubuntu-24.04`:** every job in `release.yml`, `runtime.yml` and
  `dlssnr.yml`, the compatibility matrix and guards, and the Rust workflow's
  `preflight`, `msrv`, `sources`, `gui-build` and `gui` jobs.
  `tests/test_workflows.py` checks that no other job may run self-hosted.
- **Self-hosted (`FERESTRE_RUNNER_LABELS`):** only the Rust workflow's `test`
  job, in a build directory that outlives the job (see "Build caches"). It
  produces test results and nothing else. To run it hosted as well, set the
  repository Actions variable `FERESTRE_RUNNER_LABELS` to the JSON array
  `["ubuntu-24.04"]`; delete it to go back.

## Cutting a release

1. Merge to `main`. Every release workflow refuses a tag that is not
   reachable from `main`. For a runtime release, wait for the runtime build
   that the merge started on `main` to finish (Actions, *Runtime*): the tag
   then promotes its output and takes minutes (see "Runtime promotion"). A
   tag pushed while that build is still running waits for it.
2. For the launcher, check that `docs/RELEASE-<version>.md` exists. It is the
   release text, and the build stops early without it.
3. Tag and push:

   ```sh
   git tag -a v0.1.14 -m 'what changed' && git push origin v0.1.14
   # or
   git tag -a runtime-11.0.20260907 -m 'what changed' && git push origin runtime-11.0.20260907
   ```

4. When `build` has finished, approve the `publish` job in the Actions tab.
   It checks the sums, signs, and creates the GitHub release with every file
   and its `.minisig`.

To see whether a release would build without publishing anything, start the
workflow by hand with *Run workflow*. The result is unsigned.

### Bumping DLSSNR-AMD

The neural rendering package is built from an exact upstream commit, never a
branch, and published under its own tag so the launcher can pin it:

1. Edit `packages.dlssnr-amd` in `sources.lock.json` (repository, full
   commit hash, version); `python3 tests/test_source_pins.py` checks the
   shape. Optionally build it locally first with
   `scripts/build-dlssnr.sh <empty dir>` (Docker or Podman, network, tens of
   minutes). A change to `sources.lock.json` on `main` also starts the
   runtime workflow, but when the runtime pins did not move it only checks
   and repackages the dist main already built (a few minutes).
2. Merge to `main`, then tag `dlssnr-<version>` with exactly the pinned
   version; the build refuses a tag that disagrees with the lock:

   ```sh
   git tag -a dlssnr-0.0.9 -m 'DLSSNR-AMD 0.0.9 (9503f6b) for Ferestre' && git push origin dlssnr-0.0.9
   ```

3. Approve `publish`. The release is created with `--latest=false`, so the
   README's "latest release" link stays on the launcher.
4. Download `SHA256SUMS`, check it lists both tarballs and `SOURCES.txt`,
   verify the `.minisig` files, and put each tarball's URL and SHA-256 into
   the pinned constants in `crates/ferestre-core/src/neural/package.rs`. The
   launcher trusts that pinned hash, not a rebuild: the compiled Windows files
   are not byte-identical between builds.

No NVIDIA file is in the package; `scripts/build-dlssnr.sh` refuses a tarball
holding a model or a weight file derived from one, and
`tests/test_no_nvidia_urls.py` keeps any URL of one out of the repository. That
guard runs in `.github/workflows/guards.yml` on every push and pull request,
with no path filter, so a docs-only change is checked too.

### Runtime promotion

A runtime tag normally ships the dist a push to `main` compiled, instead of
compiling it again. `scripts/runtime-inputs.py` makes that decision, and
`tests/test_runtime_inputs.py` tests every rule against API responses
recorded from this repository.

- **What the runtime is built from.** The *inputs digest* is a SHA-256 over a
  canonical JSON of the repository and commit of Proton, Wine, xgameruntime
  and vkd3d-proton from `sources.lock.json`, and the git blob id of
  `scripts/build-runtime.sh`, which holds every clone, configure and make
  step. Proton's commit pins its SDK image and every other submodule. A lock
  edit that leaves those four pins alone (a dlssnr or client pin) and a change
  to the workflow file keep the digest, so they do not compile anything.
- **`plan`** computes the digest and looks for a run that may be promoted. It
  accepts only a run of `.github/workflows/runtime.yml` in this repository
  that was a `push` to `main` (never a pull request, tag, manual run or other
  workflow), whose `build` job succeeded, at a commit that is an ancestor of
  the tag, whose own `sources.lock.json` and build script hash to the same
  digest (recomputed from that commit, not read from the artefact's name),
  with exactly one unexpired artefact named `runtime-dist-<digest>` that has a
  recorded SHA-256. The newest such run wins. If none has finished but one is
  still running with the same digest, a tag waits for it (up to five hours).
  Otherwise the tag builds the runtime itself, from main's compiler cache.
- **`build`** (only when nothing was found) runs `scripts/build-runtime.sh` and
  uploads `dist.tar.zst` with a `manifest.json`: the digest and its inputs,
  the Proton commit actually built, the SDK image, and the run id, commit,
  ref, event and repository it was built by. Only a push to `main` names the
  artefact `runtime-dist-<digest>` (kept 30 days); any other run names it
  `runtime-dist-run-<run id>`, which `plan` never accepts.
- **`package`** downloads the zip through the API, checks its SHA-256
  against the digest GitHub recorded at upload (read from the API, not passed
  between jobs), and checks every manifest field against the run it came from
  and the pinned Proton commit. Only then does it unpack, run the checks on
  those exact bytes (inherited descriptors, Wayland drivers, display modes in
  the SDK image), write the capability manifest from the tag's
  `titles/capabilities.toml`, and make the `.tar.xz` with `SHA256SUMS`.
- **`publish`** signs that, as before. The release notes name the run that
  compiled the dist and the inputs digest.

So a promoted dist was compiled from `main`-merged sources, by a `main` push,
restoring only a cache `main` saved: the same basis a tag build has.
`main` itself reuses a dist the same way, so a push that only moves the
dlssnr pin costs `plan` and `package`, a few minutes.

### Build caches

A cold runtime build takes about two hours on a hosted runner, and 12-15
minutes with a warm compiler cache. The runtime workflow keeps Proton's
compiler cache (ccache) between runs:

- Only a push to `main` that compiles the runtime saves the cache (key
  `runtime-ccache-v3-<digest>`; the `restore-keys` prefix finds the newest
  one when the pins moved). It always builds the pinned Proton commit, from
  reviewed fork sources. A manual run may build any Proton ref, whose build
  scripts could write anything into the cache, so it restores the cache and
  saves nothing; so does a tag. What a signed release restores was therefore
  compiled only from sources that were merged to `main`.
- `main` runtime builds are never cancelled: a cancelled build saves no cache
  and leaves nothing to promote. Pushes queue, and GitHub keeps only the
  newest waiting run.
- `CCACHE_DIR` is `/home/runner/.ccache` and the build tree is under the
  workspace, the same paths on every hosted runner. Both matter: the cache
  service only finds a cache saved at the same path, and ccache misses
  every object compiled in a build tree at another path (Proton sets
  `CCACHE_BASEDIR` to each source directory, but the object directories stay
  absolute). This is why the self-hosted runners, one Unix user and home
  directory each, never hit.
- Only the newest runtime cache is kept: the `prune-ccache` job deletes older
  ones. Its token can delete caches and nothing else.

The Rust workflow's `test` job, the only one on the self-hosted host, does
not use the GitHub cache there. Each runner is its own user, its `CARGO_HOME`
goes into every cache key and version, and so no runner ever restored what
another saved. Instead it builds in a directory in the runner's home that
outlives the job, `~/.cache/ferestre-ci/<name>`
(`.github/scripts/ci-target-dir.sh`), through a `target` symlink at the top of
the checkout, which checkout's clean step removes before the next job. It is
emptied when it grows past its cap or the disk runs low.

- Pull requests from this repository write to that directory, and `main` and
  `dev` pushes build in it after them. It only ever feeds CI test results.
- The release and runtime workflows never use it, and never run on that host.
  `tests/test_workflows.py` checks both.
- Hosted Rust jobs restore the GitHub cache everywhere and save it only on
  `main`, because a cache saved for a pull request, another branch or a tag
  can only be read there, and every copy counts against the repository's
  10 GB. The launcher release does not use it: a tag cannot read a cache
  keyed to another job, and saving one per tag would only take space.

## Verifying a download by hand

With the `RW...` public key from `signing.rs`:

```sh
minisign -Vm ferestre-runtime-11.0.20260907.tar.xz -P 'RW...'
```

The output ends with
`Trusted comment: file:ferestre-runtime-11.0.20260907.tar.xz version:11.0.20260907`.
Check that both the file name and the version are what you expected.

## Rotating the key

The launcher trusts exactly one key. A runtime signed with a new key is
refused by every launcher built with the old one, and the other way round. So
a rotation has two halves: get launchers that know the new key out to users,
and make sure the runtimes they will download carry signatures with the new
key.

### Planned rotation

1. Make a new pair (step 1 above) and put its `RW...` line into
   `RELEASE_PUBLIC_KEY` (step 2).
2. Replace `MINISIGN_SECRET_KEY` and `MINISIGN_PASSWORD` in the `release`
   environment with the new key.
3. Cut a launcher release (`v*`). `sign-release.sh` requires the launcher
   being published to contain the signing key, so this release is signed with
   the new key.
4. Re-sign the current runtime release, so that the new launcher can install
   it. On a trusted machine, with the new secret key:

   ```sh
   (
   set -euo pipefail
   tag=runtime-11.0.20260907; version=${tag#runtime-}
   old_key='RW...old...'                  # the old public key, as signing.rs had it
   new_key=$PWD/ferestre-release.key       # the new secret key
   mkdir "$tag" && cd "$tag"
   gh release download "$tag" --pattern '*.tar.xz' --pattern SHA256SUMS
   sha256sum -c SHA256SUMS || { echo "SHA256SUMS does not match"; exit 1; }
   # Check every file against the OLD public key, and its trusted comment
   # against this file and release, before anything gets a new signature:
   # only what was really released may be signed again.
   for f in *.tar.xz SHA256SUMS; do
       gh release download "$tag" --pattern "$f.minisig"
       comment=$(minisign -V -Q -m "$f" -P "$old_key") ||
           { echo "$f does not verify against the old key"; exit 1; }
       [ "$comment" = "file:$f version:$version" ] ||
           { echo "$f is signed as '$comment', not file:$f version:$version"; exit 1; }
   done
   # Only now, and only once all of them passed, sign with the new key.
   for f in *.tar.xz SHA256SUMS; do
       minisign -S -s "$new_key" -m "$f" -t "file:$f version:$version"
   done
   gh release upload "$tag" ./*.minisig --clobber
   )
   ```

   The subshell stops at the first failure without closing your terminal, and
   nothing has been uploaded when it does. Find out why the release does not
   match what was signed before trying again.

   Launchers from before the rotation will refuse this runtime from then on,
   and their users have to update the launcher. Wait until the new launcher
   release is out and packaged before you do this step.
5. Delete the old secret key everywhere, backups included, once nothing needs
   it any more.

### If the key is compromised

Do the same steps immediately, and in this order:

1. Remove the secret from the `release` environment, so nothing more can be
   signed with it in CI.
2. Make the new pair and put its public key into `signing.rs`.
3. Release the launcher.
4. Re-sign the runtime, checking the files against `SHA256SUMS` from a release
   you know is intact, not just against the old signature.

Say in the release notes that the key changed and why. Old launchers will
refuse runtimes signed with the new key, and that is the intended result:
signatures made with a compromised key must stop being trusted.

Accepting two keys at once during a transition would need a change to
`signing.rs`. The launcher does not do that today.
