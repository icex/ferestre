# Joining a multiplayer server

State of the investigation into joins that hang at "Locating server /
generating world". Written from captures, not from reasoning: every claim
below has a log behind it.

## What works

A **local Bedrock dedicated server** (same version as the client, 1.26.45)
accepts us completely, and this is the finding that reframed everything:

    Player connected: examplegamer, xuid: 2533270000000000
    Player Spawned:   examplegamer xuid: 2533270000000000, pfid: 0000000000000000

That was with `online-mode=true`, which turns on **both** Xbox identity
validation and the encryption handshake. So the identity chain we mint is
genuine -- the server verified the real XUID -- and the encryption handshake
completes. The client then streams chunks and spawns.

Direct joins to a server added by address, and joins from the history list,
also work.

## What does not

Joining a **featured server from the menu** hangs. Against Hive
(51.77.4.103) the capture shows:

* RakNet handshake completes: `0x05 -> 0x06 -> 0x07 -> 0x08`, then
  `ConnectionRequest -> ConnectionRequestAccepted -> NewIncomingConnection`.
* The client uploads its login: **325 KB in 345 fragments**. Bedrock logins
  carry skin data, so the size is not necessarily wrong.
* The server answers with a small game batch (65 bytes) every few seconds.
* Then it sends **`ID_DISCONNECTION_NOTIFICATION` (0x15)** -- the server
  drops us -- preceded by two small batches which presumably carry the
  reason. The client never surfaces any of it and sits on its loading screen.

Delivery is perfect throughout: inbound sequence numbers 0..147 with no
gaps, no duplicates and no NAKs in either direction. Nothing is lost.

## Ruled out, with the test that ruled it out

| Suspect | How it was cleared |
| --- | --- |
| HTTP layer | 84 responses, all 200 |
| GDK async | every operation completes; no stranded work |
| RakNet transport | ping and full handshake succeed under Wine |
| Path MTU | identical to native: 1492 fails, 1400 and under work |
| WebSockets | Xbox real-time channel upgrades successfully |
| AES-256-CFB8 | byte-identical to OpenSSL on a known vector |
| ECDH P-384 + raw secret | byte-identical to an independent computation, including the byte reversal Windows applies |
| ntsync | hangs identically with `PROTON_NO_NTSYNC=1` |
| Xbox privileges | `XUserCheckPrivilege` grants everything |
| GDK dialogs | zero `XGameUi` calls during the hang |
| Safety warnings | both `do_not_show_multiplayer_*_safety_warning` set; no change |
| NetherNet / WebRTC | no STUN (`2112a442`) and no DTLS (`16fefd`) in any capture |
| Packet loss | zero gaps, zero retransmits, zero NAKs |

## Where this stands

The server is refusing us at the application layer, after a technically
perfect connection. The next step is the disconnect reason, which is in
those last two batches. The datagram tracer now captures 256 bytes per
datagram rather than 48 so they can be read: the payloads are compressed,
and encrypted only after the handshake, so packets before that point are
readable directly.

Note for whoever picks this up: neither debugger works on this process. gdb
hangs enumerating its threads on both sync backends, and winedbg crashes
internally. The instruments in `tools/` exist because of that.

## 2026-09-06 — the freeze is a use-after-free in our own async-op list

The join freeze reproduces byte-for-byte (same stack addresses across runs), so
it is deterministic rather than a race that merely looks like one.

### What the frozen process actually looks like

It is a *partial* deadlock. The RakNet threads stay alive the whole time — the
server keeps getting ConnectedPong replies and never sees us drop — while the
game's main thread burns zero CPU. That is why it reads as "frozen" rather than
"disconnected", and why chasing the network layer led nowhere.

Recovering the real stacks needed new tooling (`tools/winestack.c`): gdb hangs
enumerating threads on this process, winedbg faults internally (`c0000005`), and
eu-stack only ever shows the unix side, because Wine's syscall dispatcher
restores the unix frame and a CFI unwinder follows it out of the PE world. The
scanner instead finds the dispatcher's syscall frame on the unix stack by its
iret signature (`CS = 0x33`, `SS = 0x2b`), reads the saved PE `rip`/`rsp` from
it, and walks the PE stack keeping only values preceded by a `call`.

Three threads were parked, each inside `RtlAcquireSRWLockExclusive`:

| lock | parked threads | `owners` | `exclusive_waiters` |
|------|---------------|----------|---------------------|
| `0x2254998`  | 1 (main thread)            | 0 | 1 |
| `0x1d356dc8` | 1                          | 0 | **0** |
| `0x20dab2d8` | 1 (our delayed-callback thread) | 0 | 6 |

`struct srw_lock` is `{ short exclusive_waiters; unsigned short owners; }`, so
`owners == 0` means **unheld**. Every one of these threads is parked on a lock
that is free, permanently — the state was stable across repeated samples. The
lock pointer comes from the saved `rbx` at `R + 0x40`; that offset validates
itself, because `R + 0x60` then holds the caller slot, which reads
`msvcp140!_Mtx_lock+0x49` on every thread as predicted.

The middle row is the tell: a waiter adds 2 to `exclusive_waiters` *before* it
parks, so a parked waiter with that field at 0 is not a state the algorithm can
produce. The lock word is not a lock any more.

### Wine's synchronisation is not at fault

Worth stating explicitly, because it was the obvious suspect. `tests/srwlock_stress.c`
hammers SRW locks under this exact build — 24 threads, 200k iterations each,
mixing exclusive, shared and `SleepConditionVariableSRW` — and passes clean. The
alert primitive is also correct: `NtAlertThreadByThreadId` latches a pending
alert with `InterlockedExchange(futex, 1)`, which `NtWaitForAlertByThreadId`
consumes without sleeping, so there is no wake/park window to lose.

### The actual defect

`async_ops_cs` protects the *list* of async operations, not the objects on it.
Every user looked an operation up under the lock, dropped the lock, and then
used the object:

```c
EnterCriticalSection( &async_ops_cs );
op = find_async_op( async );
LeaveCriticalSection( &async_ops_cs );

if (op) async_op_do_work( op );        /* op may already be freed */
```

Meanwhile `XAsyncGetResult` does `list_remove( &op->entry )` and `free( op )`,
and `XAsyncComplete` frees XAsyncRun operations the same way. Four call sites
had this shape. Nothing stopped a pool thread from being inside the provider
while another thread freed the operation under it.

That explains the lock words exactly: the freed block is reused almost at once,
and when a C++ object carrying a `std::mutex` lands in it, the mutex word is
overwritten while threads are parked on it. Nobody will ever release a lock that
no longer exists, so they park for ever. The same corruption is the likely
source of the intermittent `read of address 0x8` faults, which predate this
change and appear in older logs too.

The fix gives the operation a lifetime: the list holds one reference and each
user takes its own (`acquire_async_op` / `release_async_op`), so it is freed only
when the last user is done.

### Two things ruled out, with evidence

- **A GDK warning dialog for unrated servers.** The title calls *no* `XGameUi`
  API in any traced run; the warning is drawn by the game itself.
- **Delayed callbacks firing against a closed queue.** Plausible on its face —
  we never cancelled pending work and always passed `canceled = FALSE` — but of
  201 queues that received delayed callbacks, exactly **one** was ever closed,
  and its callbacks were submitted before the close. A cancel-on-close change
  was written, passed synthetic tests, and then crashed the title at the menu;
  it is not in the tree. Fixing it is worth doing for contract correctness, but
  it is not this bug.

### Tracing gap worth closing

`XTaskQueueCreate`/`CreateComposite` trace the *out pointer*, not the handle
they produce, so queue identities cannot be correlated across a log. That is why
the dispatch mode of the queue in the freeze could not be established.

## 2026-09-06 (later) — corrected: it is unserialised callback delivery

Two corrections to the entry above, both found by checking the method rather
than the conclusion.

**The lock readings were wrong.** The lock pointer was taken from the stack slot
at `R + 0x40`, on the reasoning that `RtlAcquireSRWLockExclusive` keeps it in
`rbx`. It does — but `push %rbx` at function entry saves the *caller's* `rbx`,
and the lock pointer is only moved into the register afterwards. That slot never
held the lock. Everything derived from it — "three locks free with waiters
parked", the lost-wakeup theory — was an artefact.

The right source is the saved PE register file. Wine's syscall dispatcher writes
a `struct syscall_frame` on the unix stack (`rbx` at +0x08, `r12` at +0x50,
`rip` at +0x70), and `RtlWaitOnAddress` keeps the waited-on address in `r12`.
Better still, its local `futex_entry` holds both the address and the waiting
thread's id; that id read `0xe4` for the main thread, matching the `00e4:`
prefix in the trace, which confirms the recovery.

Read properly, the main thread's mutex is `owners=1, exclbit=1` — **held**, not
free. An ordinary deadlock, not a Wine defect.

**The use-after-free is real, but it is libHttpClient's state, not ours.** The
chain:

1. Thread A (one of our delayed-callback pool threads, running a libHttpClient
   callback) holds mutex `0x222afe8`.
2. It blocks acquiring a second mutex whose memory has since been **freed and
   reused** — the bytes at that address now read `{"users":[]}`, an HTTP
   response body. Nobody will ever release a lock that no longer exists.
3. The game's main thread then blocks on mutex `0x222afe8`, held by thread A.
4. Everything stops. RakNet keeps answering pings, so the server never sees us
   drop and the game just sits there.

The cause is that a GDK task queue port is a **serial** execution context — the
runtime runs one callback at a time on it, and a title dispatching a Manual
queue itself necessarily runs them one after another. We handed every callback
straight to the Win32 thread pool, so one queue's callbacks ran on as many
threads as the pool offered. During a server join ten different threads were
submitting to a single queue; two callbacks ran at once, one finished the HTTP
call and freed it while the other was still inside, and the freed block was
immediately reused.

The fix queues callbacks per task queue and drains each with a single runner, so
callbacks on one queue never overlap while still never being parked (this title
never calls `XTaskQueueDispatch`). A delayed callback joins its queue's FIFO when
its timer expires, so it is serialised too.

`tests/xgr_tests.c` checks this directly, and the check is not vacuous: against
the previous build it reports `peak 2` and fails.

The async-op refcounting from the earlier entry stays — that race was found by
reading the code and is real regardless — but it was not what froze the game.

## 2026-09-06 (later still) — the trigger: an unimplemented networking API

`XNetworkingQuerySecurityInformationForUrlAsync` returned `E_NOTIMPL` and never
completed its async block. In every frozen run the trace shows that stub firing
and `XTaskQueueCloseHandle` immediately after it — the caller taking the failure
and tearing the operation down.

With it implemented, the URLs it is asked about are:

```
wss://rta.xboxlive.com/connect
wss://signaling-tm-northeurope.fran...
```

The Xbox Live Real-Time Activity socket and the multiplayer signalling socket.
libHttpClient queries a URL's security requirements before opening a WebSocket,
so failing the query fails the connect, and libHttpClient's connect-failure
teardown destroys the WebSocket while other threads still reference it. That is
where both symptoms come from:

- a thread parks for ever on the WebSocket's freed mutex (its memory reused, so
  it reads as `{"users":[]}`) while holding a mutex the game's main thread then
  wants — the freeze;
- `HC_WEBSOCKET_OBSERVER::~HC_WEBSOCKET_OBSERVER` erases itself from the
  observer map of an already-freed `xbox::httpclient::WebSocket` — the
  intermittent read of address `0x8`.

Joining from the history or by IP never opens those sockets, which is why only
the menu froze.

The crash site was pinned down by solving libHttpClient's load base from its
`.pdata` (every cited address is an exact function start) and following RTTI
from the faulting function's caller vtable to the type descriptor
`.?AUHC_WEBSOCKET_OBSERVER@@`; the same fault appears at the same RVA in an
older run at a different base.

There is nothing to pin, so the implementation reports no thumbprints and lets
the caller do the TLS validation it would do anyway —
`XNetworkingVerifyServerCertificate` already accepts the connection.

## 2026-09-06 — fixed: the initial connectivity-hint callback was never delivered

`XNetworkingRegisterConnectivityHintChanged` returned `S_OK` and never called
the callback, on the reasoning that connectivity never changes here so the
callback would never fire. But the first callback is not a change notification —
it is how the runtime hands over the *current* hint, and libHttpClient caches
`networkInitialized` from it. With no callback the cached value stayed false,
and libHttpClient refuses every WebSocket connect while it is false:

```
libHttpClient!HCWebSocketConnectAsync:
    _Mtx_lock(NetworkState + 0x38)
    cmp   byte [NetworkState + 0xa8], 0     ; cached networkInitialized
    jne   connect                            ; only proceeds when set
    _Mtx_unlock
    mov   eax, 0x89235007                    ; -> "connect submit failed"
```

Plain HTTP never consults that flag, which is why sign-in, the profile, Realms
and every ordinary request worked perfectly while everything WebSocket-shaped
failed. Minecraft's server list needs two WebSockets —
`wss://rta.xboxlive.com/connect` and the multiplayer signalling host — so a join
from the menu always failed inside libHttpClient's connect path, while joining
from the history or by IP (which open neither) always worked.

The failure itself was survivable; libHttpClient's *reaction* to it was not. Its
connect-failure teardown destroys the WebSocket while an observer still holds a
reference, so `HC_WEBSOCKET_OBSERVER::~HC_WEBSOCKET_OBSERVER` later runs
`lea rcx,[rbx+0x108]; call _Mtx_lock` against freed memory. Depending on what
that block has been reused for, it either faults (the intermittent read of
address `0x8`) or parks for ever on something that is no longer a lock — and
because the thread holds a Minecraft mutex at the time, the game's main thread
blocks behind it and the whole title freezes while RakNet keeps answering pings.

The fix delivers the current hint once at registration, on the thread pool
rather than inline: the caller registers from inside its own network-state lock
and its handler takes that same non-recursive lock.

Verified end to end: three consecutive automated runs join The Hive from the
in-game server list and reach the lobby.

### Two corrections to earlier entries here

`tools/winestack.c` was reporting addresses in unnamed modules one page too low.
Wine maps a PE's header page from the file and its `.text` anonymously one page
higher, so a code address lands in a region with no path, and naming it after
that region's start yields `ImageBase + 0x1000` as the base. Several frames were
therefore attributed to the wrong functions, some mid-instruction. Corrected, the
deadlocked chain reads `HCWebSocketCloseHandle` ->
`HC_WEBSOCKET_OBSERVER::~HC_WEBSOCKET_OBSERVER` -> `_Mtx_lock(WebSocket+0x108)`,
which is the freed mutex named exactly. The tool now resolves an anonymous code
region to the mapping that ends where it begins.

Its `call [reg+disp32]` check was also off by one — that form is 6 bytes, so it
occupies `b[10..15]`, not `b[9..14]` — which could admit false frames.
