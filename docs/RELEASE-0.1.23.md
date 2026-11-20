# Ferestre 0.1.23

- The install dialog lists mounted, writable drives alongside the default library and custom folder. It shows the final game folder and free space before downloading. A queued install waits when its selected drive is unavailable.
- Downloads can fetch up to eight verified HTTP ranges at once. Paused installs can resume with their saved progress.
- Ferestre recognizes Easy Anti-Cheat packages by their launcher and package files. Their licensed images are available at their installed paths for the protected launch in a private mount namespace.
- The matching Proton runtime adds HTTP/2 to WinHTTP. Gears of War: E-Day uses it to reach its Xbox PC configuration and login services. HTTP/1.1 remains available, including for WinHTTP WebSockets.
