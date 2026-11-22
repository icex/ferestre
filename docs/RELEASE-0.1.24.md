# Ferestre 0.1.24

Pairs with runtime 11.0.20261122.2. The game fixes below need that runtime; **Settings → Choose default runtime** selects it once it is installed.

## Games

- Hogwarts Legacy is playable: Xbox sign-in and two hours of play, confirmed by the player. The runtime now waits for the game's sign-in and Store licence callbacks before it reports them done, which the game's recipe turns on. A first launch can close while the game prepares its shaders; start it again, the work already done is kept and later starts are not affected.
- Batman: Arkham Knight is playable, checked by a person. Its first save no longer spins for ever after New Game Options: the runtime opens a save provider when the game asks without a configuration id.
- Control and Crash Bandicoot N. Sane Trilogy are playable, checked by a person.
- Dead by Daylight is playable, checked by a person. The runtime fetches the Store's Collections ID key through Ferestre's client service, so the game loads its Store content instead of showing CANNOT CONNECT TO STORE, and Wine accepts the game's repeated first web socket receive, which crashed it at Loading Characters. Real-money purchases are not supported.
- Death Stranding Director's Cut is playable, with issues. The runtime tells the game which parts of its package are installed, so it no longer stops with "Internal system error occured" a few seconds after launch. It reaches the title menu, starts a New Game and plays the opening; this was checked off-screen with scripted input, not yet by a person. Online features fall back to Offline Mode ("Network features cannot be used ... Error Code: 10000").
- All six are in the [compatibility list](COMPATIBILITY.md#the-matrix).

## The window

- Pages open much faster. Cover art is decoded off the main thread, scaled to the size it is shown at and cached; background updates no longer redraw the whole page; file system checks are remembered between redraws; and the install dialog scans drives in the background.
- A new layout built around the art: Home has a carousel of what you played last, updates and Game Pass titles, followed by shelves and featured cards. The sidebar lists every installed title with its status, and a Compact sidebar setting (in the account menu and in Settings) turns it into a rail of icons.
- Notifications stack in a column instead of waiting in a queue, and a bell keeps this session's notifications for the ones that went by unseen.
- Page switches ease in, and the window blurs behind dialogs where the graphics hardware handles it well; slow renderers get a plain fade.
- Newly installed titles show their art straight away, and the carousel never shows a blank banner.
- Library tabs keep their header in place, and a page no longer leaks memory each time it is left.

## Launching

- The launch settings editor shows a title's settings as switches and menus, and Reset to defaults puts back the shipped recipe (your old settings are kept as a `.bak` file).
- Pressing Play on a title that is already running asks first: leave it running, or close it and start again. Close It gives the game 15 seconds to exit before it is ended.
- `ferestre run` refuses to start a second copy in the same prefix and exits with code 4. The refusal is written to the title's launch log, and when no terminal shows it, as from a Steam shortcut, it also appears as a desktop notification.

## Runtime 11.0.20261122.2

- xgameruntime waits, with a time limit, for every completion callback delivered on the thread pool when a recipe sets `XGR_WAIT_FOR_ASYNC_CALLBACKS=1` (Hogwarts Legacy).
- Game saves open for a null configuration id, using the save storage MicrosoftGame.config names (Batman: Arkham Knight).
- `XStoreGetUserCollectionsIdAsync` returns the Store's Collections ID key, fetched by the matching client service (Dead by Daylight).
- `XPackageEnumerateChunkAvailability` reports every chunk and language of the package as ready (Death Stranding Director's Cut).
- WinHTTP treats a repeated first web socket receive, with the same buffer, as the same request (Dead by Daylight).
