# Ferestre 0.1.14

Pair this launcher with [runtime 11.0.20261023.1](https://github.com/icex/ferestre/releases/tag/runtime-11.0.20261023.1). Both updates are needed for the MCC campaign fix. The AppImage and portable archive include the matching Xbox service.

## Halo: The Master Chief Collection (#22)

- MCC now discovers installed campaign packages and obtains each package's own authenticated Store licence. A campaign's licence and revocation remain independent of the base game.
- Installing MCC automatically prepares its six companion packages: Reach, CE, Halo 2, Halo 3, ODST and Halo 4. Play also prepares missing packages before launching; complete installations are reused. A licence refusal is reported while other licensed content remains usable.
- Installed add-ons show **Installed** and identify their base game. They need no Set up step, standalone launch recipe or separate prefix. Launch MCC to play them.
- The tested recipe selects the package's official **Anti-Cheat Disabled** executable.
- The runtime retains async provider data through Cleanup, fixing a deferred-callback crash on exit. Asynchronous queue termination notifications also use the Completion port.

For an existing installation, update both the launcher and runtime, then press **Play**. If MCC is pinned to an older runtime, select the new runtime or Automatic in **Choose runtime**.

## Move location (#23)

- Installed games have **Move location** in More actions, with a standard arrow icon.
- Selecting or editing a folder automatically checks space and shows the target. **Move** stays disabled during validation, for an invalid destination or when space is insufficient.
- Copying and verification report progress and can be cancelled. Cross-drive copies are read back and verified before the installation record changes and the original is removed. Same-drive moves stage hard links.
- Running games and their in-use content packages cannot be moved. Prefixes and saves keep their paths; launches follow the updated installation record.

## Verification and limits

- Launcher: 522 workspace checks passed, 2 ignored; Clippy with warnings denied passed; all 9 recipe-validator tests passed. Hosted checks passed for both merged PRs, including the real GTK Move workflow.
- Retail relocation: Forza Horizon 6's 171.5 GB installation moved from Btrfs to NTFS through the GUI, passed file verification, reached the driving world and relaunched from its new location.
- MCC: all six content packages installed. Desktop Play automatically restored a missing Halo 4 package; every restored file matched by SHA256. Installed content rows required no Set up.
- Independent retail checks reached CE's Pillar of Autumn, Reach's Winter Contingency and Halo 2's Armory. Save/Quit and resume were checked; Reach movement and firing worked. Normal exit and ordinary Window Close returned code 0 without access violations.
- Independent Halo 3, ODST and Halo 4 visual checks remain incomplete because the desktop locked during testing. The local user reports that the formerly missing campaigns now work. Full campaign completion, online modes and the reporter's Fedora/Nvidia environment are unverified.
- Runtime regression checks passed: 18 related-package, 25 related-licence, 371 general, 75 Manual queue, 74 PlayFab, 77,735 concurrent-runtime and 59 catalog checks, plus iterator and HTTP fixtures. Account-role checks passed (109 plus a credential-log probe); service checks passed (45, 1 ignored).

An existing forced-ExitProcess save test intermittently reported 3/4 writes before the final full quiet run passed. Its writer and expectations are unchanged; the documented limitation concerns killing an in-progress write, separately from normal Save/Quit. See [MCC investigation notes](https://github.com/icex/ferestre/blob/v0.1.14/notes/mcc-campaign-packages.md).
