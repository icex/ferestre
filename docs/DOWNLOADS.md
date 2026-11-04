# Downloads, incremental updates and controls

Home shows featured artwork and recent games. My Library, Game Pass and Installed
use large cover cards; pressing a card starts its primary action. The always-visible
More menu includes Game details, launch settings and other actions. Installed games
with a confirmed update show an Update available badge on their cover, including
while running; current installs and unknown update checks show no badge. Search is
centered above the library. On
narrow windows, Show navigation opens the sidebar. Touch controls have at least
44 logical pixels; scrolling uses GTK's touch and kinetic scrolling support.

After opening Game Pass, covers for its full catalogue preload in the background
with four bounded workers. Visible pages take priority and each cover appears
individually as it arrives; cached covers are reused across pages and restarts.

A game's whole card is dimmed under a centered animated spinner and Starting…
while its launcher prepares it. Once the executable from that specific launch is
observed, the overlay shows Running with separate Resume and Stop controls.
Resume focuses that existing game's window without launching another copy.
Stop opens a confirmation dialog with Cancel selected first; D-pad/stick and
A/cross select the response, while B/circle cancels. Stop also works during
startup, and force stop during shutdown requires another confirmation. Failed launches and game
exits return the card to Play; other running games retain their own state.

SDL2 controllers navigate the focused Ferestre window with D-pad or the left
stick. A activates, B closes a dialog/menu or returns to navigation, Y focuses
search, LB/RB change sections, View opens navigation and Menu opens the focused
game's options. In navigation, ↑/↓ chooses an item; → or A enters its games.
The stick moves spatially through banners, section tabs, game cards and paging.
The focused control has a persistent green outline and scrolls into view.
Xbox Home/Guide or PlayStation PS on any connected mapped controller brings the
main Ferestre window forward. Controllers plugged in after startup are discovered
by SDL event pumping,
including while another application has focus. It restores controller focus
without pressing the highlighted game's action. View/Share/Create continues to
open navigation. On KDE Wayland, a one-shot KWin activation request targets only
Ferestre's own process, app ID and main-window role; it is unloaded afterward. Other
desktops use GTK window presentation, subject to the compositor's activation
policy. A desktop or Steam mapping that consumes Home before SDL must let that
button reach Ferestre.
SDL controller type detection displays PlayStation cross/circle/triangle,
L1/R1 and Share/Create for DualShock/DualSense controllers. The footer explains
the available controls for the current focus. Press Options/Menu without a title
selected to show the full mapping. Steam Input can expose a physical controller as a virtual Xbox device;
in that case the exposed device type determines the default labels. A stick dead zone and
held-direction repeat avoid accidental movements. Buttons held when another
application gives Ferestre focus do not activate anything. Devices are never
grabbed and no desktop-wide input is injected. SDL2 is loaded optionally from
the system; missing SDL2 leaves normal touch, mouse and keyboard navigation.
Controller activation of a text field opens an in-app keyboard. Done commits its
draft and Cancel/B leaves the field unchanged; password drafts stay masked. The
install destination button opens an in-app folder browser with Home, Parent,
hidden-folder and Use This Folder controls. Account and other option selectors
also use controller-navigable choices. Settings includes Quit Ferestre.

Keyboard: arrows/Tab navigate, Enter/Space activate, F6 changes pane, Ctrl+F
searches and F11 toggles fullscreen.

Fullscreen has its own handheld layout: navigation moves to a large top bar,
window controls and the desktop sidebar give way to game covers, and the first
game receives controller focus. Promotional banners are omitted so complete
cards fit at 1280×720 and 1280×800. Controls grow to 48 logical pixels, with
larger captions and controller hints. Enter/Exit fullscreen in the header, F11,
or Fullscreen in navigation switches modes. The choice is remembered for the
next launch; Exit fullscreen remains available by touch.

## Queue and pause

Install and Update add a game to Install Queue. Only one queued transfer runs at a
time. Pause closes the backend's HTTP stream and checkpoints completed data;
Resume obtains the remaining ranges. Stop retains checkpointed data. Queued
and active entries become paused after restarting Ferestre: a saved queue
never starts downloads automatically in a new session. Queue entries record
product, chosen destination and operation, with no credentials or download
URLs. Existing clients without the structured control protocol retain Stop
and indeterminate/legacy progress; Pause appears only when advertised.

The backend stores partial output under `.ferestre-transfer`, bound to the
signed package and file layout. It syncs each completed window. On resume it
reverifies every complete saved page against the new package's authenticated
hash tree; a corrupt suffix is discarded. An incomplete final plaintext page
is fetched again. No journal is trusted to assert that local data is authentic.

## Game updates

Updates reconstruct one changed file at a time under `.ferestre-transfer`, using bounded 8 MiB windows and parallel range
requests.
Matching old page hashes identify local candidates even when offsets moved.
Every reused page is checked against the new package; plaintext candidates are
re-encrypted in memory with the licensed new parameters before comparison.
Modified, missing and unverifiable data is downloaded and checked by Ferestre's
existing authenticated package hash tree. Signatures and package identity
checks remain on the normal metadata path. Keys are never written to staging state.

Completed files are synced and renamed individually. A durable
`.ferestre-update-incomplete-v1` barrier blocks game launches until every file
and the new package metadata have committed. It survives process death and is
independent of the GUI queue. This protects against launching a mixed version;
it is not whole-game rollback. Original game files are retained until their
replacement finishes. Resume refreshes package authentication and CDN links.
The progress bar measures rebuilt data; downloaded and locally reused bytes
are shown separately.

## Diagnostics

Install Queue → View download diagnostics and `ferestre diagnostics --json` show
recent reason codes and HTTP statuses. The bounded private log keeps only
allowlisted metadata, never raw output, arguments, account identifiers, tokens
or CDN URLs. Failures distinguish authentication, license, metadata/integrity,
HTTP response, interrupted connection, disk space and client startup stages.

The queue, diagnostics reader and presentation remain separate MIT launcher
code using the GPL client process protocol.

## Verification scope

Offline GUI tests exercise queue ordering, pause/resume controls, cooperative
stop and recovery after restarting the real window. Backend fixtures exercise
authenticated page reuse, changed keys and hash trees, incomplete-update launch
guards, failed transfers and checkpoint recovery. SDL virtual controllers and a process-private adapter test
the input mapping, automatic button glyphs, pane/section navigation and actual
focus indicator pixels without touching physical devices. These checks do not
qualify a full retail game update or physical touch/controller hardware.
