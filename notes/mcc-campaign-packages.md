# MCC campaign packages

Issue [#22](https://github.com/icex/ferestre/issues/22) reports that MCC asks to
install Halo CE even after Ferestre has installed its content package.

The full base package contains the campaign assets. The small Halo CE and
Reach packages carry their own Store identities and declare both an
`AllowedProduct` for MCC and a `MainPackageDependency` on `Microsoft.Chelan`.
Installing them does not give them separate executables or prefixes.

The old runtime returned no entries for
`XPackageEnumeratePackages(Content, ThisAndRelated)`. Ferestre's install
records were never passed to that runtime, so independently installed add-ons
were invisible even though their files and manifests were present.

The launcher now supplies the completed related installations from its records,
including packages in another library. It checks both manifest relationships,
excludes partial/stale installations and holds their shared title locks for the
lifetime of the base game. Restart the game after installing additional content. An update,
removal or Move of the add-on cannot
invalidate an in-use package snapshot.

The runtime checks the relationship again, assigns each content installation a
33-byte SDK-compatible identifier, and reports its Store identity, display
metadata and version. `ThisOnly` does not include content belonging to a related
package. Enumeration stops when its callback returns false. A mount of an
enumerated identifier resolves to that add-on's directory rather than to the
base game's directory. Installation discovery remains separate from Store
ownership and licensing. MCC subsequently acquires a licence for each package
and queries entitled Game/Durable products. Both paths now ask the credential
service for that package's own authenticated content licence. The launcher
forwards product/content bindings only from related completed install records;
the service keeps separate grant caches. Its service namespace includes the
binding, so installing another add-on cannot reuse an older service inventory.

The entitled query reports only granted packages, retains product data across
pages, honours the requested kinds and stops at callback cancellation. Related
licence handles and licence-lost notifications have separate polling state;
an add-on refusal or revocation does not seed or invalidate the base game's
licence. This needs the matching client patch `0028` as well as runtime `0035`.
There is no ownership inferred from the package files.

MCC's official No-EAC executable is available in the package. The recipe names
its exact package path, because the client's executable selection is case
sensitive. The ordinary bootstrapper was also observed rejecting its child
executable path before reaching the game.

## Verification

`RELATED_ONLY=1 tests/run-tests.sh` runs the focused native package checks in
an isolated prefix. It covers two matching content packages, an unrelated
package, scope filtering, callback cancellation, identifiers and mount paths.
The installed baseline fails the related-package and mount checks; the candidate
passes them. `cargo test --workspace` covers launcher discovery, including a
separate library, a wrong parent/dependency and an unfinished installation.

The focused checks also cover an installed-but-denied add-on, per-package
acquisition, kind filtering, paging after closing an earlier handle, revocation
and its licence-lost callback, and the base licence remaining valid.
The Rust service tests cover malformed product/content bindings and independent
cached grants, including Store sign-out.

Use `WINEDEBUG=-all,+xstore` to inspect package acquisition and authenticated
entitlements without enabling the per-frame task-queue trace. `+xpackage` adds
package discovery, mounts and installation-monitor calls.

## Retail procedure

Install MCC (`9N6VPDLQDVT5`) with Ferestre. Its recipe prepares the six
companion campaign packages automatically after installation, and Play repairs
missing content before building the runtime inventory. Already complete
packages are reused. Each download still asks Microsoft for its own licence;
an unavailable add-on is reported and does not prevent other licensed
campaigns from running. Content preparation is not recursive.

The package ids are Reach `9N9RNPBLR7X3`, CE `9P84QCTDHD04`, Halo 2
`9MWMBQ1GNK6K`, Halo 3 `9NN5X5KW23VT`, ODST `9N007VX9RGXZ` and Halo 4
`9NN6VS9SPW2R`. The base archive contains the campaign assets but does not
replace these content identities. Installed manifests declare a parent and no
executable: the window now shows **Installed / Content for MCC**, without
Set up, a standalone launch recipe or a separate prefix.

Keep the Store account that has access selected; the Xbox Player may be a
different account. Content installed during an already running game becomes
visible on the next launch.

The local baseline was package 1.3528.0.0 with runtime
11.0.20261017.2-accounts. With CE 1.1367.0.0 installed it still displayed
"Halo: CE is not installed" at Campaigns / Halo CE / Quickstart / Normal.
This reproduces the reported symptom on the local runtime, rather than claiming
the reporter's Fedora/Nvidia/runtime-18.2 environment was reproduced.

The candidate has reached Halo CE's Pillar of Autumn and Reach's Winter
Contingency through those campaign menus, and both Save/Quit actions returned
to the front end. The original exit failure in a deferred provider callback is fixed by retaining
provider data through Cleanup; the candidate then exited with code 0 and no
access violation.

The normally installed AppImage was then launched through its desktop entry.
Its Play action resumed both CE and Reach checkpoints; Reach was tested on foot
with movement and firing (the magazine count changed from 32 to 28). Both
Save/Quit actions returned to the front end. Quit to Desktop ended the game
and restored the launcher row to Play. Moving CE while MCC was running was
refused by its related-title lock. Online modes and complete campaign
playthroughs are untested.


## Deferred provider data and queue termination

MCC's queue-cleanup provider retains its `XAsyncProviderData` pointer and hands
it to a later termination callback. That callback reads `context` (offset 24)
and `async` after `DoWork` has returned. The old runtime constructed the data
on the calling stack for each provider invocation. The retail trace caught the
later callback reading a null context and faulting while releasing it.

Patch `0037` retains provider data in the operation through Cleanup, matching
[Microsoft's AsyncLib state](https://github.com/microsoft/libHttpClient/blob/main/Source/Task/AsyncLib.cpp).
The deferred-provider regression fails two checks on the installed baseline
and passes on the candidate. The candidate then exited MCC with code 0 and no
access violation.

Patch `0036` separately routes asynchronous termination notifications through
the Completion port after accepted work has drained, as required by
[Microsoft's termination contract](https://learn.microsoft.com/en-us/xbox/gdk/docs/reference/system/xtaskqueue/functions/xtaskqueueterminate).
The recipe uses strict Manual dispatch. Native checks verify that a Manual
notification is never invoked inline, is not delivered by Work dispatch, and
runs once on the Completion-dispatching thread. Running-callback and
self-termination cases remain covered. This routing correction alone did not
resolve MCC's retained-data fault.

The complete final runtime suite passed: 18 package checks, 25 related licence
checks, 371 general checks, 75 manual-queue checks, 74 PlayFab checks, the
concurrent runtime checks and 59 catalog checks. An existing forced-exit save
test intermittently reported 3/4 writes before the final quiet run passed;
the save writer already documents that a write killed in progress by
`ExitProcess` cannot be recovered. This is distinct from waiting for an in-game
Save/Quit. No changes to that writer or test expectation are included here.
