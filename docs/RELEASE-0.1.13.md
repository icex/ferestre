# Ferestre 0.1.13

This launcher update pairs with runtime **11.0.20261018.2**. The new runtime
is required for separate Store and Player accounts and for the GameInput
gamepad fixes, and carries the runtime fixes from the code review below. The
controller fixes in the launcher work with older runtimes.

**Releases are now signed.** From this release on, the launcher only installs
runtimes signed with the project's release key, and refuses older runtimes
unless asked. Runtime 11.0.20261018.2 is the first signed runtime; earlier
runtimes stay usable once installed, but `ferestre install-runtime` no longer
downloads them.

## Online status

- While a game started through Ferestre runs, from the window, a Steam
  shortcut or `ferestre run`, your Xbox friends see you playing it. It is
  reported with the game's own identity, and nothing is reported when no game
  is running.
- On by default after sign-in. **Show me online** in the account menu turns
  it off per Player account.
- A dot next to your name at the bottom left shows what Xbox reports for you,
  Online or Offline; its tooltip names the game. With no game running it
  says Offline, since Ferestre does not show you online on its own.

Verified live with three real accounts and Stardew Valley: the status
appeared while the game ran and was removed after it quit. See
[PRESENCE.md](PRESENCE.md).

## Signed game package headers

- The client now checks Microsoft's signature on every game package header
  before it trusts anything in it, and refuses a header that does not verify,
  turns integrity checks off, or names a different package than the package
  service did.
- Neither Ferestre nor the client ships the key. The first install fetches it
  anonymously from Microsoft Gaming Services in the Store and keeps it only if
  its SHA-256 matches the pinned fingerprint. Until then, or if fetching fails,
  installs go on with a warning; `XODUS_REQUIRE_SIGNED_PACKAGE=1` refuses
  them instead. See [PACKAGE-SIGNATURES.md](PACKAGE-SIGNATURES.md).

Verified against the headers of eleven installed retail games, and with a
fresh Stardew Valley install with signatures required.

## Code review fixes

A review of the whole codebase found 114 distinct problems (6 high, 31
medium, 77 low), all fixed here.

- **Runtime downloads** no longer fail on slow links after 60 seconds; they
  time out only when the transfer stalls.
- **Remove and `uninstall`** only ever delete one title's own folder. Empty or
  `..` ids, home, the games folder and folders holding other titles are
  refused.
- **Install to** is a library folder: each title gets its own subfolder.
- **Stop** asks the game to close and waits 15 seconds before ending it. A
  second press ends it at once.
- **Downloads** are checked page by page against the package's hash tree, and
  a failed download or extract fails instead of reporting success. Package
  file names can no longer write outside the install folder. Partial
  downloads show as incomplete.
- **Recipe installs** no longer fail when the only runtime is a managed one.
  One bad recipe file no longer hides the rest.
- **Xbox service:** service tokens are bound to the title they were issued
  for, and the licensing audience is checked against a fixed set of hosts.
- **Sign-in window** only accepts messages from Microsoft login pages. Store
  tickets are no longer written to logs, and the keyring file never falls
  back to the shared temp directory.
- **Runtime (11.0.20261018.2):** task queue termination and cancellation,
  token requests kept off title queues, save, UI and certificate fixes in
  xgameruntime; GameInput hotplug and ordering, ntdll and kernelbase fixes,
  and a heap overflow in `concrt140` fixed in Wine.

## Controllers (#10)

- DualShock 4 and DualSense pads reach GDK titles without Steam. Wine took
  Sony pads as raw HID, which games cannot read as gamepads; they now go
  through SDL. Your own `PROTON_ENABLE_HIDRAW` / `PROTON_DISABLE_HIDRAW`
  setting, a recipe's, or Proton's `user_settings.py` still wins. This needs
  the host's SDL2 (`sdl2-compat` on Arch and Fedora, `libsdl2-2.0-0` on
  Ubuntu). `ferestre doctor` checks for it and the launch warns without it.
- Steam Input works for Ferestre shortcuts: the game keeps the
  `SteamGameId`/`SteamAppId` Steam needs to apply the shortcut's controller
  layout. `--steam` is now only needed for the overlay and Remote Play.
- Steam no longer shows a game as running after it quits. The Xbox service no
  longer keeps the AppImage mounted; the last game using it stops it.
- GameInput (in the new runtime): "any pad" readings follow the pad used last,
  reading timestamps mark when the input changed, and titles that ask for the
  older GameInput device interface get it.

Verified with an emulated DualShock 4 over USB and Bluetooth, with and
without the real Steam client. Not verified: a physical pad, Fedora, and
plugging a pad in after a game has started.

## Sign-in on systems without WebKitGTK (#8)

- The Xbox client's sign-in window needs WebKitGTK 4.1. Without it, the
  client could not start at all, and Sign in failed with "error 127" (seen on
  Debian 13 with KDE). Ferestre now says so in a banner and offers
  **Install…**, which installs the right package through your system's
  package manager (PackageKit, as used by Discover and GNOME Software) with
  the usual password prompt. Without PackageKit it shows the exact command.
- Fedora Atomic and Bazzite are pointed at `rpm-ostree install`; SteamOS gets
  an explanation instead of a command that would not survive an update.
- `ferestre doctor` reports missing client libraries and the command.

Verified on Debian 13 with the real PackageKit. Not verified: the password
prompt on a real KDE desktop.

## Separate Store and Player accounts (#9)

- Accounts settings choose the **Store** account (purchases, Game Pass) and
  the **Player** account (gamertag, achievements, saves) separately. Both are
  shown before launch, and each player gets its own save prefix.
- A running game keeps the pair it started with; changing the selection
  applies to the next launch.
- Installed games that the Store account cannot license say so in the list,
  and launch failures show the client's own reason.
- Game Pass lists and counts only the games the Store account can play, in
  the market of your locale rather than always the US.
- Split pairs need the new runtime; older runtimes refuse them instead of
  launching with the wrong account.

Verified with two real accounts on Forza Horizon 6 and Minecraft for Windows.
Not verified: Minecraft Dungeons II with a split pair, reloading the player's
save after a restart, and a subscription lapsing during play.

## Other fixes

- **Add All to Steam:** the Installed list's header adds every installed game
  to Steam in one go, after asking. Games already in Steam are updated in
  place. Two titles that share a name, such as two editions, now get a
  shortcut each instead of sharing one.
- The video dependency check looks where Proton does, including Steam's own
  FFmpeg, so Steam users no longer get a false warning.
- Signing out somewhere else (`xodus-cli logout`, an expired token) no longer
  leaves the previous account and its library on screen.
- Refresh pressed while another task runs is kept and runs afterwards.
- A refresh during a disk-usage scan no longer starts a second full scan.
- Add to Steam only uses an AppImage path for Ferestre's own AppImage.
- Library rows keep secondary actions behind one menu; the Accounts dialog
  is rebuilt with Adwaita groups.
