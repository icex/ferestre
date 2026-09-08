# Appx and Msix Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Discover and safely retrieve unencrypted Appx/Msix Store packages, then identify their application model.

**Architecture:** A new core archive inspector has no network dependency and does not extract archives. A separate FE3 module owns SOAP composition, parsing, TLS scoping, and verified file delivery. The CLI presents inspection independently from launch support.

**Tech Stack:** Rust 2021, `zip`, `ureq`, `serde`, XML text fragments constrained to known Microsoft manifest and SOAP shapes.

**Spec:** `docs/superpowers/specs/2026-09-08-appx-msix-delivery-design.md`

## Global Constraints

- Keep Xodus as a child process; do not link its GPL-3.0-only crates.
- Do not add decryption support for E-prefixed packages.
- Do not extract an untrusted archive during classification.
- Preserve the MSIXVC install and launch path unchanged.
- Keep public fixtures free of account ids, tokens, and licenses.

---

### Task 1: Archive inspector

**Files:**
- Create: `crates/ferestre-core/src/appx.rs`
- Modify: `crates/ferestre-core/src/lib.rs`
- Modify: `crates/ferestre-core/Cargo.toml`

**Interfaces:**
- Produces `inspect_archive(path: &Path) -> Result<Package>`.
- Produces `Package { identity, version, executable, entry_point, kind }`.

- [ ] Write tests that build a zip with `AppxManifest.xml` and assert a
  FullTrust entry point is `DesktopBridge`, a non-FullTrust entry point is
  `Uwp`, and a missing manifest is rejected.
- [ ] Run `cargo test -p ferestre-core appx` and observe unresolved module/API
  failures.
- [ ] Add the minimal `zip` dependency and inspector with a 4 MiB manifest
  ceiling; read the manifest by exact name and classify without extraction.
- [ ] Re-run the focused test and `cargo test -p ferestre-core`.
- [ ] Commit `feat: inspect Appx and Msix package manifests`.

### Task 2: Inspection command

**Files:**
- Modify: `crates/ferestre-cli/src/main.rs`

**Interfaces:**
- Consumes `appx::inspect_archive`.
- Produces `ferestre inspect-package PATH` text and JSON results.

- [ ] Write a command parsing test for `inspect-package /tmp/package.appx`.
- [ ] Run the CLI test and observe the missing command failure.
- [ ] Add command dispatch with JSON-only stdout and no launch action.
- [ ] Re-run `cargo test -p ferestre-cli`.
- [ ] Commit `feat: expose Appx package inspection`.

### Task 3: FE3 metadata discovery

**Files:**
- Create: `crates/ferestre-core/src/fe3.rs`
- Add: `crates/ferestre-core/tests/fixtures/fe3-*.xml`
- Modify: `crates/ferestre-core/src/lib.rs`

**Interfaces:**
- Consumes a `WuCategoryId` and a host-scoped transport.
- Produces `Update { id, revision, files }` where each file has URL, digest,
  block map, and `encrypted` status.

- [ ] Write parsers against scrubbed SOAP envelopes and assert that an
  encrypted file cannot become a delivery candidate.
- [ ] Run the focused tests and observe missing module/API failures.
- [ ] Implement SOAP composition and response parsing; add FE3 root trust only
  to the FE3 agent.
- [ ] Re-run the core suite and a manual FE3 metadata request for one owned,
  unencrypted package.
- [ ] Commit `feat: discover Appx delivery through FE3`.

### Task 4: Verified unencrypted delivery

**Files:**
- Modify: `crates/ferestre-core/src/fe3.rs`
- Modify: `crates/ferestre-cli/src/main.rs`

**Interfaces:**
- Consumes a validated unencrypted FE3 file location.
- Produces an atomically written archive after SHA-1 verification.

- [ ] Write tests covering digest mismatch removal and no download for encrypted
  locations.
- [ ] Run focused tests and observe the missing downloader failure.
- [ ] Implement bounded, atomic download and SHA-1 comparison, then invoke the
  archive inspector before accepting the result.
- [ ] Run workspace tests, formatting, clippy, and one manual unencrypted
  package delivery.
- [ ] Commit `feat: download verified unencrypted Appx packages`.
