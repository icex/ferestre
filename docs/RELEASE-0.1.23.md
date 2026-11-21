# Ferestre 0.1.23

- The install dialog lists mounted, writable drives alongside the default library and custom folder. It shows the final game folder and free space before downloading. A queued install waits when its selected drive is unavailable.
- Downloads can fetch up to eight verified HTTP ranges at once. Paused installs can resume with their saved progress.
- Ferestre recognizes Easy Anti-Cheat packages by their launcher and package files. Their licensed images are available at their installed paths for the protected launch in a private mount namespace.
- The matching Proton runtime adds HTTP/2 to WinHTTP. Gears of War: E-Day uses it to reach its Xbox PC configuration and login services. HTTP/1.1 remains available, including for WinHTTP WebSockets.
- Gears 5 (Xbox PC) is playable, campaign and multiplayer. The matching runtime supports the legacy GDK XPackage interface Gears 5 asks for at startup, and the launcher supplies Easy Anti-Cheat's Linux library, which the Xbox PC package lacks. Ferestre keeps the library only if its SHA-256 matches, and shows it in the game's EAC folder for the launch only; the installed package is not changed.
- Ferestre downloads Valve's Proton Easy Anti-Cheat and BattlEye runtimes from the same Lutris URLs that Lutris and Heroic use, keeps them only if their SHA-256 matches, and passes them to Proton. A value you set yourself, or one a recipe sets, takes precedence.
- The runtime notifies XPackage installation monitors when an installation is complete, as Windows does.
- Pressing Play on a title the selected runtime cannot start now says why and offers Choose runtime. Before, the press did nothing.
