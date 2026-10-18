# Online status

While a game you started through Ferestre is running, your Xbox friends see
you playing it, as they would on Windows. This page explains what they see,
when, and how to turn it off.

## What friends see

- **While a game runs:** your Player account shows as playing that game, for
  example "Playing Forza Horizon 6". Ferestre reports it using the game's own
  identity, the TitleId and MSAAppId in the game's `MicrosoftGame.config`, so
  friends see the real game.
- **When no game is running:** Ferestre reports nothing. It is not an Xbox
  title, and it does not borrow another title's identity to show you as
  "online". Friends see whatever your other devices report, such as a console
  or the Xbox app.
- **When the game exits:** Ferestre tells Xbox the game has stopped, within a
  few seconds.

Ferestre does not offer Away or "Appear offline". Whether Xbox accepts them
from anything but its own apps is unconfirmed.

## On by default, per Player

Online status is **on by default** once an account is signed in. To turn it
off, click your name at the bottom left and switch off **Show me online**.

- The choice is kept for each Player account separately, in
  `~/.config/ferestre/presence.json`.
- Switching it off while a game is running ends the status within a few
  seconds. The game keeps running.
- Switching it on while a game is running starts the status for that game.
- If the settings file cannot be read, online status counts as off.

From a terminal, the same file turns it off:

```json
{"schema":1,"players":{"<player profile id>":{"share_games":false}}}
```

## The status dot

The dot and the text beside your name show what Xbox reports for you, and
nothing else. Ferestre asks Xbox every five minutes, whenever you open the
account menu, and right after a game's status is sent or taken down.

| Text | Meaning |
|---|---|
| Playing *game* | Xbox lists the game started through Ferestre |
| Online | Xbox shows you online, but not with that game, for example from a console |
| Away | Xbox shows you away on another device |
| Offline | Online status is on, but Xbox shows nothing right now |
| Off | Online status is off, and Xbox shows nothing |
| Unknown | Xbox could not be asked |

The dot shows Xbox's answer even when the switch is off. If you are signed in
on a console, friends can see you there, and so does the dot.

What Ferestre is doing for a running game is in the dot's tooltip, under
Xbox's answer:

- "Reporting *game*; Xbox does not list it yet": the game's status was sent
  and accepted, and the next check should show it.
- "Reconnecting: …": Xbox could not be reached. Ferestre keeps trying.
- "Not shared: …": Xbox refused, or this game has no identity to report. It
  is not tried again until the game is started again or the switch is turned
  off and on.

The account menu says the same under **Show me online**.

## Where it works

Every launch goes through `ferestre run`, so all of these report the game:

- the Play button in the window
- a Steam shortcut made by Ferestre
- `ferestre run` in a terminal

Closing the Ferestre window does not stop a running game, so the game's status
stays until the game exits.

Online status needs a Xodus client that lists `presence-v1` in
`xodus-cli account-profiles --json`. With an older client the switch is greyed
out and asks you to update the client. Games still launch as before.

## Limits

- **Only games started through Ferestre.** A game started some other way is
  not reported.
- **One game per Player at a time.** If the same Player starts a second game
  while one is showing, only the first is reported.
- **Signing out:** signing the Player account out stops the status first, so
  Xbox is told the game stopped while the account can still say so.
- **Changing accounts while a game runs:** the running game keeps the Player
  it started with, and so does its status. The dot follows the new Player.
- **Games that report their own status:** a game that also reports presence
  through the Xbox service may show up on two devices. This is still being
  measured.

## For the curious

The launcher never holds an Xbox token. `ferestre run` starts the client's
`presence run` next to the game and closes its stdin when the game exits. The
client prints its progress as JSON lines, which `ferestre run` passes on to its
own stderr as `:: presence {...}` lines. That is also how the window follows a
game it started. The window can stop or restart a running launch's status by
sending SIGUSR1 (stop) or SIGUSR2 (follow the saved setting) to that
`ferestre run` process. The signal goes to the process only, never to its
group, which also holds the game. Each launch also says which Player its
status belongs to, so signing out reaches the right games.
