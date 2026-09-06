# Title recipes

One TOML file per title, named after its Microsoft Store product id. This is
the data version of what currently lives in a `scripts/launch-*.sh` and a
section of `docs/RECIPES.md`, and the reason for it is arithmetic: adding a
title today means editing five places, which is what caps the project at three
titles.

The schema is small because the variance is small. `scripts/launch-gdk.sh` is
75 lines and most of it is undoing environment that Steam injects — the same
for every title. What actually differs between the three titles here is the
executable path, the directory layout, one file substitution, where the saves
are, and what the title needs from the runtime. That is the whole schema. None
of the three needs a single environment variable of its own.

## Naming

    titles/9NBLGGH2JHXJ.toml

The file is named after the product id and nothing else, because that is the
identifier a user actually has: it is the 12-character code in the Store URL
(`https://apps.microsoft.com/detail/9NBLGGH2JHXJ`), it is what `get-game.sh`
takes, and it is in the title's own `MicrosoftGame.Config` as `<StoreId>` if
the game is already installed. A recipe whose id nobody has written down is not
usable by anybody else, so the validator refuses one.

`titles/capabilities.toml` is not a recipe; it is the registry of capability
names, described below.

## The schema

`schema = 1` at the top, then these tables. Unknown keys are an error: a
misspelled key would otherwise silently read as "not set".

### `[title]` — required

| key | type | | meaning |
|---|---|---|---|
| `product-id` | string | required | 12 characters, `[0-9A-Z]`. Must match the filename. |
| `name` | string | required | As the Store shows it. Goes in the compatibility matrix. |
| `slug` | string | required | Lowercase; names the install directory, the prefix and the log unless overridden. |
| `publisher` | string | optional | |
| `package-identity` | string | optional | `<Identity Name=…>` from `MicrosoftGame.Config`. Useful when hunting save paths. |
| `title-id` | string | optional | The hex `<TitleId>`. Appears in save container names and in Xbox Live traces. |
| `install-size-gb` | number | optional | Measured on disk after decrypting, not the Store's download figure. |

### `[install]` — optional

Where the title and its prefix live, relative to `XODUS_GAMES_DIR`.

| key | type | | meaning |
|---|---|---|---|
| `dir` | string | optional | Default: the slug. Forward slashes, must stay inside the games directory. |
| `dir-env` | string | optional | An environment variable that overrides `dir`, for compatibility with the existing scripts. |
| `prefix` | string | optional | Default: `<slug>-proton`. |
| `prefix-env` | string | optional | As `dir-env`, for the prefix. |

### `[launch]` — required

| key | type | | meaning |
|---|---|---|---|
| `executable` | string | required | Backslash path **relative to the install directory**, as Windows writes it. Use a TOML literal string (single quotes) so backslashes stay backslashes. |
| `arguments` | array of string | optional | |
| `environment` | table | optional | Extra environment, on top of what the launcher sets for every title. Only put something here that is genuinely this title's. |

The executable named in `MicrosoftGame.Config` is not always the one to run —
Expedition 33 declares a wrapper — so this is the observed path, not a copy of
the manifest.

### `[runtime]` — required

| key | type | | meaning |
|---|---|---|---|
| `requires` | array | required | Capability names the title cannot start without. |
| `wants` | array | optional | Capabilities whose absence degrades it but does not stop it. |

Names come from `capabilities.toml`; anything else is an error. See
"Capabilities, not versions".

### `[[setup]]` — optional, repeatable

Something that has to be done to the installed files before the title works.
Not a shell hook: each `action` is a named operation a runner implements, so
that a recipe cannot execute arbitrary code and a runner can say honestly
whether it supports a recipe.

| key | type | | meaning |
|---|---|---|---|
| `action` | string | required | Currently only `substitute-xcurl` (what `scripts/fix-xcurl.sh` does). |
| `when` | string | required | `after-install` (also after any update or re-download, which restores the original files) or `every-launch`. |
| `because` | string | required | Why. This is what someone reads when they wonder whether it is still needed. |
| `dir` | string | optional | Backslash subdirectory the action applies to. Default: the install root. |

Adding an action means implementing it in the runner and documenting it here.
That friction is deliberate.

### `[[saves]]` — optional, repeatable

| key | type | | meaning |
|---|---|---|---|
| `kind` | string | required | `wgs` (Windows Gaming Services containers) or `files` (an ordinary directory). |
| `windows` | string | required | Where it is on Windows, starting with a `%VARIABLE%`. |
| `prefix` | string | required | The same place inside the Wine prefix, relative to `pfx/`, so it starts with `drive_c/`. |
| `note` | string | optional | |

A `*` may stand for one whole path segment (Bedrock's per-user directory).

### `[status]` — required

The compatibility matrix is generated from this, so it is the part to be
pedantic about.

| key | type | | meaning |
|---|---|---|---|
| `state` | string | required | See below. |
| `summary` | string | required | One sentence, used verbatim in the matrix. |
| `stops-at` | string | required for `menus` and `broken` | Precisely where it stops. "Does not launch" is not an answer. |
| `blocked-by` | string | required for `broken`, and only then | `title-protection`, `runtime-gap`, `client-gap`, `unknown`. |
| `last-verified` | date | required unless `untested` | A bare TOML date, `2026-09-06`. An entry with no date is a rumour. |
| `verified-with` | string | optional | How it was checked, so someone else can repeat it. |

### `[[issues]]` — optional, repeatable

| key | type | | meaning |
|---|---|---|---|
| `symptom` | string | required | What the person sees. |
| `cause` | string | required | |
| `fix` | string | required | Say "none known" if there is none. |
| `log-match` | string | optional | A regular expression that identifies this issue in a launch log, so a failed run can name itself. |

`docs/RECIPES.md` has a general troubleshooting table for things that are not
title-specific; that belongs there, not here.

## Capabilities, not versions

A recipe never names a Proton or Wine version. Pinning one freezes a title to a
build that will be superseded, and it answers the wrong question: what a title
needs is not "Proton 11.0-3" but "a runtime that can load an image from a
memfd". So a recipe declares behaviour, and any runtime that provides that
behaviour can run the title.

The names live in `titles/capabilities.toml`, one entry per capability, each
with what it is, which patch provides it, and what you see when it is missing.
That last field is the useful one: it turns a capability list into a diagnosis.

A runtime build is expected to ship the list of what it provides — one name per
line, `#` comments allowed — and the validator will check a recipe against it:

    titles/validate.py --runtime "$XODUS_PROTON_DIR/xgdk-capabilities.txt"

No runtime build generates that file yet. This is the contract it has to
satisfy when one does; until then the check is opt-in and the registry is
maintained by hand against `patches/`.

Only list a capability you have evidence the title uses — a log line, a
documented failure. Omitting one is not a claim that the title does not use it;
inventing one puts a false entry in the matrix. Forza Horizon 5 requires
exactly one capability here, because that is all it has ever been observed to
reach before dying.

## Status values

| value | means |
|---|---|
| `playable` | Plays. No known blocker. |
| `playable-with-issues` | Plays, but at least one `[[issues]]` entry bites in normal use. Requires an issues entry. |
| `menus` | Starts and draws its UI; gameplay is not reachable. |
| `broken` | Does not get to a window. |
| `untested` | Written from documentation, never run. |

`playable-with-issues` is deliberately not folded into `playable`. A matrix that
calls both of them "works" is exactly the kind of claim this project cannot
afford to make.

## What a runner does with a recipe

The resolution rules, which reproduce what `scripts/launch-*.sh` do today:

    install dir  = ${dir-env}    if set, else $XODUS_GAMES_DIR/${install.dir or slug}
    prefix       = ${prefix-env} if set, else $XODUS_GAMES_DIR/${install.prefix or "<slug>-proton"}
    log          = $XODUS_GAMES_DIR/<slug>-launch.log

So `titles/9NBLGGH2JHXJ.toml` is the same thing as

    scripts/launch-gdk.sh "$XODUS_GAMES_DIR/bedrock/game" 'Minecraft.Windows.exe' \
        "$XODUS_GAMES_DIR/bedrock-proton" "$XODUS_GAMES_DIR/bedrock-launch.log"

which is what `scripts/launch-bedrock.sh` already runs. The recipes describe
the existing behaviour; they do not propose a new one. Nothing in `scripts/`
reads them yet.

## Validating

    titles/validate.py                     # every recipe here
    titles/validate.py 9NBLGGH2JHXJ.toml   # just this one
    titles/validate.py --matrix            # the compatibility matrix, as markdown
    titles/validate.py --runtime FILE      # also check against a runtime's capability list

Needs Python 3.11 or newer (`tomllib`). Exit status is 1 if anything failed, so
it can go straight into CI. It checks types, unknown keys, path shapes,
capability names, that regular expressions compile, that filenames match
product ids, that no two recipes claim the same id or slug, and the rules that
tie `[status]` together — a `broken` title must say what blocked it and where it
stopped, a `playable` one must not claim to stop anywhere, a verification date
must not be in the future.

## Deliberately not in the schema

- **A Proton or Wine version.** See above.
- **Shell hooks.** `[[setup]]` names operations a runner implements. A recipe
  is data, and data that can run commands is not data.
- **Download URLs or keys.** The client fetches the title with the user's own
  licence; a recipe has nothing to say about that beyond the product id.
- **A list of patches.** Recipes point at capabilities; capabilities point at
  patches. A title should not have to care how a capability is implemented.
- **Anything per-machine.** Paths are relative and resolved from `XODUS_*`
  environment variables, so a recipe is the same file on everyone's disk.
