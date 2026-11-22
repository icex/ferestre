#!/usr/bin/env bash
# Decide which Rust workflow jobs a pull request needs, and write the answers
# to $GITHUB_OUTPUT as rust=, gui= and sources= (true or false).
#
#   ci-changes.sh pull_request    # in a checkout of the PR's merge commit
#   ci-changes.sh <other event>   # everything runs
#
# On a pull request the checkout is the merge commit GitHub made, so its first
# parent is the base branch as it is now, and HEAD^1..HEAD is exactly what the
# PR would change. A push to main or dev, and a manual run, always run every
# job: that is where a job skipped here would be caught.
#
# The `test` job is not decided here. It always runs: it holds the Python,
# shell and packaging checks for every path the workflow triggers on.
set -euo pipefail

out=${GITHUB_OUTPUT:-/dev/stdout}
event=${1:?event name}

if [ "$event" != pull_request ]; then
    printf 'rust=true\ngui=true\nsources=true\n' >> "$out"
    echo "$event: every job runs"
    exit 0
fi

rust=false gui=false sources=false
while IFS= read -r path; do
    case "$path" in
        # The workflow and these helpers change how every job runs.
        .github/workflows/rust.yml|.github/scripts/ci-*.sh)
            rust=true gui=true sources=true ;;
    esac
    case "$path" in
        crates/*|Cargo.toml|Cargo.lock|rust-toolchain|rust-toolchain.toml)
            rust=true gui=true ;;
    esac
    case "$path" in
        # The window's tests, their drivers and fixtures. Recipes too: the
        # tests read titles/ through the GUI.
        tests/gui_*.py|tests/controller_driver.py|tests/test_gui_actions.py|\
        tests/run_wayland_gui.py|tests/wayland_focus.py|tests/fake_packagekit.py|\
        tests/run_gui_ci.py|tests/gui_ci.json|tests/fixtures/*|titles/*)
            gui=true ;;
    esac
    case "$path" in
        # The source lock, the pinned forks, the scripts that check them and
        # the fixtures the client's offline tests use.
        sources.lock.json|.gitmodules|third_party/*|scripts/check-sources.sh|\
        scripts/source-pins.py|tests/run-tests.sh|tests/build-license-service-fixture.sh|\
        tests/license_token_fixture.py|tests/related_license_fixture.py)
            sources=true ;;
    esac
done < <(git diff --name-only HEAD^1 HEAD)

printf 'rust=%s\ngui=%s\nsources=%s\n' "$rust" "$gui" "$sources" >> "$out"
echo "changed:"
git diff --name-only HEAD^1 HEAD | sed 's/^/  /'
printf 'rust (msrv)=%s gui=%s sources=%s\n' "$rust" "$gui" "$sources"
