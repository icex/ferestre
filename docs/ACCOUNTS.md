# Accounts

Open the profile menu in Ferestre, then choose **Accounts**.

## Use one account

Choose **Sign in** and complete Microsoft's sign-in window. Use the same account for both **Store account** and **Player account**.

If sign-in cannot open, follow Ferestre's missing-library prompt or check the [WebKitGTK dependencies](DEPENDENCIES.md#webkitgtk-for-sign-in).

## Choose Store and Player accounts

The **Store account** supplies game purchases, Game Pass subscriptions and content licences. The **Player account** supplies the Xbox identity used for play, achievements, friends and saves.

1. Add the accounts you want through Microsoft's sign-in window.
2. Choose the **Store account** whose games you want to use.
3. Choose the **Player account** you want to play as, then apply the selection.
4. Check the account names shown in Ferestre before launching a game.

Changes apply to the next launch. A running game keeps the account pair it started with. Selecting different accounts does not grant extra game access; Microsoft and publisher licence checks still apply.

Separate Store/Player selection requires a compatible runtime. Current signed runtime **11.0.20261107.1** supports it; select a suitable runtime in Settings or the game's options if you previously chose an older one.

## Saves and account changes

Each Player has separate game save storage. Switching Player accounts can therefore show a different set of saves. Keep using the same Player to continue that Player's progress; changing the Store account does not select another Player's saves.

Cloud-save and online features depend on the game. Check its [compatibility notes](COMPATIBILITY.md#the-matrix) for the tested scope.

## Refresh purchases or subscriptions

After a purchase, renewal or account change, use **Reload library** in the profile menu or the top-right **Refresh** button. A failed refresh keeps the last complete list so an interrupted request does not empty the library.

My Library lists account titles and local installations. Browse the separate Game Pass section for the current public PC catalogue. A subscription download does not grant permanent ownership after a game leaves the service.

## Online status

**Show me online** in the profile menu controls whether Ferestre reports the Player's activity while a game it launched is running. With no game running, the status can show Offline. See [online presence](PRESENCE.md) for details.

## Sign out

Choose the account's **Sign out** action and review the in-app confirmation. **Cancel** is selected first and keeps the account signed in. Check which account you selected before confirming.

For runtime and launcher implementation details, see the [original account-role design](ACCOUNT-SHARING-PROPOSAL.md).
