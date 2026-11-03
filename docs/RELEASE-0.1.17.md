# Ferestre 0.1.17

The **Update available** badge now sits at the lower-left corner of the game cover, keeping the top-right actions menu clear on narrow desktop and handheld cards.

GUI regression checks verify the badge fits inside its card and stays below the menu in desktop and fullscreen layouts. The launch/loading/Stop checks continue to exercise the full card.

This patch retains the features and runtime from [0.1.16](https://github.com/icex/ferestre/releases/tag/v0.1.16). The signed 0.1.16 GUI also passed the real SDL to native KDE Wayland focus test on the unlocked desktop with kernel-emulated PS4, PS5, Xbox 360, Xbox One and generic Xbox controllers, PS4 reconnect and controller Stop. These are simulated-device checks; physical controller qualification remains separate.
