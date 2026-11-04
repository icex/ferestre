# Ferestre 0.1.18

Running games now offer **Resume** and **Stop** across the card. Resume returns to the existing game window; Stop opens an in-app confirmation with Cancel selected by default. Home/PS brings Ferestre forward so you can switch games or stop them.

## Changes

- Controller-friendly text entry, installation folders, account selection and runtime selection. PlayStation controllers show their own button glyphs.
- **Settings → Choose default runtime** selects the default runtime; **game options → Choose runtime** overrides it for one title. Choices apply to the next launch.
- Library tabs retain focus on the selected section through redraws and background refresh. Search keeps its focus, game cards show one selection outline, and download controls remain reachable at handheld and desktop sizes.
- The sidebar footer contains only Settings, with matching mouse hover. Settings shows concise runtime information and a chooser, replacing duplicate installed-version and technical capability lists.
- Paused updates resume their saved destination directly. Cached package metadata is reused after checking the current remote header, then signatures and authenticated hashes are verified again. Damaged cache data gets one fresh retry.
- Download progress separates metadata preparation, processed data, actual game-data downloads and reused data.
- Server-rejected device credentials are renewed with the existing device key and retried once, preserving account identities and save namespaces.
- Component fixes now live in pinned public `icex` forks, with a verified source graph and no patch-application stage. README includes approved anonymous screenshots and a separate [compatibility list](COMPATIBILITY.md).

## Runtime and verification

This launcher continues to use signed runtime **11.0.20261023.1**. The initial runtime fork migration preserves the former source trees and adds no new Wine/runtime behaviour. The newly built fork runtime also passed the native regression harness; it is separate from the existing signed public runtime.

Validation includes 574 passing workspace tests, formatting and Clippy, offline client/service/authentication/download-integrity tests, and hosted GUI checks. Controller flows cover simulated PS4, PS5, Xbox and generic SDL identities. The exact local preview AppImage passed normal launch, extract-and-run, fullscreen, mouse hover, runtime selection and desktop Settings checks.

DOOM 64 and Stardew Valley launched through live Microsoft licensing in private copies using the fork runtime; Home/Resume/Cancel/confirmed Stop passed. Stardew reached its main menu and DOOM rendered its intro. A private Minecraft update resumed, reused verified data, received new data and stopped cooperatively. A live Forza update completed to **3.479.874.0** with its metadata committed and incomplete marker cleared.

These checks do not establish a full game playthrough, Forza gameplay, physical-controller qualification, cloud-save behaviour or cold-boot qualification. Wine networking was isolated during the retail launch tests to protect cloud saves.
