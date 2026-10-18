# Ferestre 0.1.13

This launcher update pairs with runtime **11.0.20261018.1**. The new runtime
is required for separate Store and Player accounts and for the GameInput
gamepad fixes. The controller fixes in the launcher work with older runtimes.

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

- The video dependency check looks where Proton does, including Steam's own
  FFmpeg, so Steam users no longer get a false warning.
- Signing out somewhere else (`xodus-cli logout`, an expired token) no longer
  leaves the previous account and its library on screen.
- Refresh pressed while another task runs is kept and runs afterwards.
- A refresh during a disk-usage scan no longer starts a second full scan.
- Add to Steam only uses an AppImage path for Ferestre's own AppImage.
- Library rows keep secondary actions behind one menu; the Accounts dialog
  is rebuilt with Adwaita groups.
