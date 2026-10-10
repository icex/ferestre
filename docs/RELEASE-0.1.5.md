# Release 0.1.5 qualification

Launcher **0.1.5** and runtime **11.0.20261010.1** must be updated together for
Halo Infinite. The runtime uses the same pinned Proton/Wine sources as
11.0.20261010, with GDK patches 0022–0026 added; no upstream upgrade is claimed.

## Included fixes

- Halo licensed images are mapped at installed paths inside a private user/mount
  namespace, preserving the installed encrypted bytes and original bootstrapper.
  The namespace survives the multiplayer-to-campaign process handoff and cleans
  up its own descendants on cancellation or launch failure.
- User, save-provider and quota async operations publish their actual result
  sizes and compatible handle payloads.
- Halo opts into owner-dispatched manual work/completion queues, shared composite
  endpoints, delayed dispatch and cancellation. Existing titles retain the
  default completion behavior.
- Analytics returns the full Windows.Desktop device family expected by PlayFab
  Party. Store product data lives until the product query handle closes.
- Recipe parsing and validation agree on the canonical `env` key and its
  documented `environment` alias.

## Live evidence and limits

| Title | Observed result | Remaining scope |
|---|---|---|
| Halo Infinite | Game Pass Ultimate package 1.4206.46191.0 reached Warship Gbraakon gameplay; player confirmed campaign, multiplayer and saving; campaign handoff/relaunch and normal process exit were observed | Intermittent offline display is unresolved; purchases, cloud sync and long-session reliability are unqualified |
| Minecraft Dungeons II | Sign-in, Store licence checks and Squid Coast gameplay worked; player confirmed saving after finishing the tutorial | Mid-tutorial restart was not a demonstrated save defect; multiplayer and cloud sync remain unqualified |
| Goat Simulator 3 | Native Party initialization succeeded after failing on the previous analytics family; clean online sign-in/session requests succeeded; Store lifetime regression passed | San Angora world loading remained stuck. Repeated unsupported save-interface requests are a lead, not a proven root cause. Gameplay, co-op, voice and saving are unqualified |

PR #1 had no save/checkpoint fix. Dungeons requires no additional save patch
based on the completed tutorial and the player's confirmation.

Halo's captured sign-in, presence, profile and session requests succeeded. The
observed HTTP 400s were image requests; they do not establish Xbox sign-in
failure or explain the intermittent offline indicator.

## Build provenance

The initial runtime asset is packaged locally from the published
`runtime-11.0.20261010` base with its GDK DLL and Unix library rebuilt from the
full pinned patch series. A checksum comparison found all other runtime binaries
unchanged. The source and complete patches remain reproducible through the
runtime workflow at this release tag.

## Verification

The release qualification includes 255 Rust workspace tests, 216 GDK checks in
compatibility mode, 46 strict manual-queue checks, five real namespace/lifecycle
tests and a manifest-inheritance shim test. The built GTK GUI passed its accessibility smoke test, including Runtime release
ordering. The AppImage mounted and reported the packaged version.
The Store lifetime regression failed
before the fix and passed afterward. Patched GDK translation units compile with
Wine's `-Werror` flags. The full ordered GDK series is checked against its pinned
upstream source and compared with the source used to build the runtime.

These checks qualify the changed contracts; they do not replace the live-game
limits above. Raw traces remain private because they contain authentication
and account data. No raw traces, saves or game binaries are part of the release.
