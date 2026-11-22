# Notes for agents working in this repository

- How to test, and the rules for doing it on someone's own machine:
  [docs/TESTING.md](docs/TESTING.md). In short: games run off-screen in a
  headless gamescope (`tools/headless-title.sh`), GUI tests run on a private
  display and D-Bus session, scratch space is under `/var/tmp`, and nothing
  reaches the real keyring or asks for credentials.
- Recipes and their schema: `titles/SCHEMA.md`. After a recipe change, run
  `python3 titles/validate.py` and
  `python3 .github/scripts/gen-compatibility.py`.
- Where the runtime sources come from and how they are pinned:
  `docs/SOURCES.md`, `scripts/check-sources.sh`.
- Issues are referenced with "Refs #N". The maintainer closes them after
  confirming with the reporter.
