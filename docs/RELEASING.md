# Releasing

Two release trains, both driven by tags and nothing else:

| Tag | Workflow | Publishes |
|---|---|---|
| `v<version>`, e.g. `v0.1.14` | `.github/workflows/release.yml` | the launcher: `ferestre-<version>-x86_64.tar.zst`, `ferestre-<version>-x86_64.AppImage`, `SHA256SUMS` |
| `runtime-<version>`, e.g. `runtime-11.0.20260907` | `.github/workflows/runtime.yml` | the patched Proton: `ferestre-runtime-<version>.tar.xz`, `SHA256SUMS` |

Every published file has a minisign signature beside it, `<file>.minisig`. The
launcher checks a runtime's signature against the public key built into it
(`RELEASE_PUBLIC_KEY` in `crates/ferestre-core/src/signing.rs`) and refuses a
runtime that is unsigned or signed by any other key.

## How signing is wired

- Each workflow has two jobs. `build` compiles everything with a read-only
  token and never sees the key. `publish` runs only for a tag push
  (`github.event_name == 'push' && github.ref_type == 'tag'`), after `build`
  succeeded. It is the only job with `contents: write` and the only job that
  names the `release` environment, so it is the only job that can read the
  signing secrets. It runs no build and no third-party action apart from
  checkout and download-artifact. All actions are pinned to a commit.
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
  add two tag rules, `v*` and `runtime-*`. Add no branch rules. A run from a
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
Rulesets**: a tag ruleset for `v*` and `runtime-*` that limits who can create,
move or delete them. The environment controls who can use the key; the
ruleset controls who can start a release at all.

## Cutting a release

1. Merge to `main`. Both workflows refuse a tag that is not reachable from
   `main`.
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

### Build caches

A cold runtime build takes a little over two hours. To avoid paying that on
every release, the runtime workflow keeps Proton's compiler cache (ccache)
between runs:

- A push to `main` that changes `patches/wine`, `patches/xgameruntime`,
  `patches/vkd3d-proton`, `patches/proton`, `scripts/write-capabilities.sh` or
  the workflow itself (which holds the Proton pin) builds the runtime on
  `main`, unsigned. That build saves the cache, and a newer push cancels an
  older one still running.
- Only that automatic `main` build saves the cache. It always builds the
  pinned Proton commit, from reviewed patches. A manual run may build any
  Proton ref, whose build scripts could write anything into the cache, so it
  restores the cache and saves nothing. What a signed release restores was
  therefore compiled only from sources that were merged to `main`.
- A `runtime-*` tag restores the cache `main` saved and saves nothing. GitHub
  lets a tag read the caches of the default branch but not another tag's, so
  merging the runtime changes to `main` before tagging is what makes the
  release fast. Tagging straight after the merge still works; the tag just
  finds the cache from the previous `main` build.
- Only the newest runtime cache is kept: the `prune-ccache` job deletes older
  ones. Its token can delete caches and nothing else.

The Rust jobs work the same way. They restore their caches everywhere, but
only save them on `main`, because a cache saved for a pull request, another
branch or a tag can only be read there, and every copy counts against the
repository's 10 GB.

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
