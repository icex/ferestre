#!/usr/bin/env python3
"""Check the title recipes in this directory, and build the matrix from them.

  validate.py                     check every recipe beside this script
  validate.py 9NBLGGH2JHXJ.toml   check only the ones named
  validate.py --matrix            print the compatibility matrix as markdown
  validate.py --runtime FILE      also check that FILE provides every required
                                  capability (the list a runtime build ships)

Recipes are data that a launcher and the published compatibility matrix are
generated from, so a mistake in one is invisible until somebody cannot start a
game. Unknown keys are errors rather than ignored: a misspelled key would
otherwise read as "not set", which is exactly the failure this format exists to
stop. The schema is documented in SCHEMA.md.

Exit status is 1 if anything failed.
"""
import datetime
import re
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent

# The honest states. "playable-with-issues" is separate from "playable" because
# a matrix that calls both "works" is the kind of claim this project cannot
# afford to make.
STATES = {
    "playable":             "Playable",
    "playable-with-issues": "Playable, with issues",
    "menus":                "Starts, menus only",
    "broken":               "Does not run",
    "untested":             "Untested",
}
BLOCKED_BY = {"title-protection", "runtime-gap", "client-gap", "unknown"}
SETUP_ACTIONS = {"substitute-xcurl"}   # a runner has to implement each of these
SETUP_WHEN = {"after-install", "every-launch"}
SAVE_KINDS = {"wgs", "files"}
RECIPE_NAME = re.compile(r"[0-9A-Z]{12}\.toml")

NUM = (int, float)
TYPE_NAMES = {str: "a string", int: "an integer", float: "a number", bool: "a boolean",
              list: "an array", dict: "a table", datetime.date: "a date (YYYY-MM-DD)"}


def type_name(kind):
    if isinstance(kind, tuple):
        return " or ".join(TYPE_NAMES.get(k, str(k)) for k in kind)
    return TYPE_NAMES.get(kind, str(kind))


def fields(err, where, table, required, optional):
    """Check presence and type of every key, and reject keys not in the schema."""
    for key, kind in required.items():
        if key not in table:
            err(f"{where}: missing required key '{key}'")
        elif not isinstance(table[key], kind):
            err(f"{where}.{key}: expected {type_name(kind)}")
    for key, value in table.items():
        if key in required:
            continue
        if key not in optional:
            err(f"{where}: unknown key '{key}'")
        elif not isinstance(value, optional[key]):
            err(f"{where}.{key}: expected {type_name(optional[key])}")


def check_windows_path(err, where, value, must_be_exe=False):
    """A path inside the package, as the title's own manifest writes it."""
    if not value:
        err(f"{where}: empty")
        return
    if "/" in value:
        err(f"{where}: use backslashes; this is a path inside a Windows package")
    if value.startswith("\\") or re.match(r"^[A-Za-z]:", value):
        err(f"{where}: must be relative to the install directory")
    if any(seg in ("", ".", "..") for seg in value.split("\\")):
        err(f"{where}: '{value}' does not stay inside the install directory")
    if must_be_exe and not value.lower().endswith(".exe"):
        err(f"{where}: '{value}' is not an .exe")


def check_relative_path(err, where, value, starts_with=None, wildcard=False):
    """A path on the Linux side: under the games directory, or inside the prefix."""
    if not value:
        err(f"{where}: empty")
        return
    if "\\" in value:
        err(f"{where}: use forward slashes")
    if value.startswith("/"):
        err(f"{where}: must be relative, not absolute")
    if any(seg in ("", ".", "..") for seg in value.split("/")):
        err(f"{where}: '{value}' does not stay inside its parent")
    if not wildcard and "*" in value:
        err(f"{where}: no wildcards here")
    if wildcard and any("*" in seg and seg != "*" for seg in value.split("/")):
        err(f"{where}: '*' may only stand for a whole path segment")
    if starts_with and not value.startswith(starts_with):
        err(f"{where}: expected a path under '{starts_with}'")


def check_capability_list(err, where, names, known):
    seen = set()
    for name in names:
        if not isinstance(name, str):
            err(f"{where}: expected capability names as strings")
            continue
        if name in seen:
            err(f"{where}: '{name}' listed twice")
        seen.add(name)
        if known and name not in known:
            err(f"{where}: unknown capability '{name}' (not in capabilities.toml)")
    return seen


def load_capabilities(path):
    """The registry of capability names a recipe may ask for."""
    try:
        data = tomllib.load(open(path, "rb"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return None, [f"capabilities.toml: {exc}"]
    problems = []
    known = {}
    for entry in data.get("capability", []):
        name = entry.get("name")
        if not isinstance(name, str) or not re.fullmatch(r"[a-z0-9]+(\.[a-z0-9-]+)+", name):
            problems.append(f"capabilities.toml: bad capability name {name!r}")
            continue
        if name in known:
            problems.append(f"capabilities.toml: '{name}' defined twice")
        # Every capability must be checkable against a real build. Without a
        # marker, `scripts/write-capabilities.sh` cannot publish it, so the
        # launcher falls back to probing and hedges about every title -- and a
        # capability nobody can verify is a promise, which is the thing the
        # manifest exists to replace.
        markers = entry.get("verify")
        if not isinstance(markers, list) or not markers:
            problems.append(
                f"capabilities.toml: '{name}' has no `verify` markers, so no build "
                f"can prove it provides it"
            )
        else:
            for i, marker in enumerate(markers):
                where = f"capabilities.toml: '{name}'.verify[{i}]"
                if not isinstance(marker, dict):
                    problems.append(f"{where}: expected a table")
                    continue
                for field in ("file", "contains"):
                    value = marker.get(field)
                    if not isinstance(value, str) or not value:
                        problems.append(f"{where}: `{field}` must be a non-empty string")
                path = marker.get("file")
                if isinstance(path, str) and (path.startswith("/") or ".." in path):
                    problems.append(
                        f"{where}: `file` is relative to the runtime directory, "
                        f"not an absolute or climbing path"
                    )
        known[name] = entry
    if not known:
        problems.append("capabilities.toml: no capabilities defined")
    return known, problems


def load_runtime_capabilities(path):
    """What a runtime build says it provides: one name per line, # comments."""
    names = set()
    for line in Path(path).read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            names.add(line)
    return names


def check_recipe(path, known):
    """Return (recipe or None, list of problems)."""
    problems = []
    err = problems.append
    try:
        data = tomllib.load(open(path, "rb"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return None, [str(exc)]

    fields(err, "(top level)", data,
           required={"schema": int, "title": dict, "launch": dict,
                     "runtime": dict, "status": dict},
           optional={"install": dict, "setup": list, "saves": list, "issues": list})
    if data.get("schema") != 1:
        err(f"schema: expected 1, got {data.get('schema')!r}")
    if problems:
        return None, problems

    title = data["title"]
    fields(err, "[title]", title,
           required={"product-id": str, "name": str, "slug": str},
           optional={"publisher": str, "package-identity": str,
                     "title-id": str, "install-size-gb": NUM})
    product_id = title.get("product-id", "")
    if not re.fullmatch(r"[0-9A-Z]{12}", product_id):
        err(f"[title].product-id: '{product_id}' is not a 12-character Store id "
            "(it is in the Store URL, and in the title's MicrosoftGame.Config as <StoreId>)")
    elif path.stem != product_id:
        err(f"filename: recipes are keyed by product id, so this should be {product_id}.toml")
    slug = title.get("slug", "")
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", slug):
        err(f"[title].slug: '{slug}' must be lowercase letters, digits and dashes")

    install = data.get("install", {})
    fields(err, "[install]", install, required={},
           optional={"dir": str, "dir-env": str, "prefix": str, "prefix-env": str})
    if "dir" in install:
        check_relative_path(err, "[install].dir", install["dir"])
    if "prefix" in install:
        check_relative_path(err, "[install].prefix", install["prefix"])
    for key in ("dir-env", "prefix-env"):
        if key in install and not re.fullmatch(r"[A-Z][A-Z0-9_]*", install[key]):
            err(f"[install].{key}: '{install[key]}' is not an environment variable name")

    launch = data["launch"]
    fields(err, "[launch]", launch, required={"executable": str},
           optional={"arguments": list, "environment": dict})
    check_windows_path(err, "[launch].executable", launch.get("executable", ""), must_be_exe=True)
    for arg in launch.get("arguments", []):
        if not isinstance(arg, str):
            err("[launch].arguments: every argument must be a string")
    for key, value in launch.get("environment", {}).items():
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            err(f"[launch.environment]: '{key}' is not an environment variable name")
        if not isinstance(value, str):
            err(f"[launch.environment].{key}: expected a string")

    runtime = data["runtime"]
    fields(err, "[runtime]", runtime, required={"requires": list}, optional={"wants": list})
    requires = check_capability_list(err, "[runtime].requires", runtime.get("requires", []), known)
    wants = check_capability_list(err, "[runtime].wants", runtime.get("wants", []), known)
    if not requires:
        err("[runtime].requires: every title needs at least loader.memfd-main-image")
    for name in requires & wants:
        err(f"[runtime]: '{name}' is in both requires and wants")

    for i, step in enumerate(data.get("setup", [])):
        where = f"[[setup]] #{i + 1}"
        if not isinstance(step, dict):
            err(f"{where}: expected a table")
            continue
        fields(err, where, step, required={"action": str, "when": str, "because": str},
               optional={"dir": str})
        if step.get("action") not in SETUP_ACTIONS:
            err(f"{where}.action: '{step.get('action')}' is not implemented by any runner "
                f"(known: {', '.join(sorted(SETUP_ACTIONS))})")
        if step.get("when") not in SETUP_WHEN:
            err(f"{where}.when: expected one of {', '.join(sorted(SETUP_WHEN))}")
        if "dir" in step:
            check_windows_path(err, f"{where}.dir", step["dir"])

    for i, save in enumerate(data.get("saves", [])):
        where = f"[[saves]] #{i + 1}"
        if not isinstance(save, dict):
            err(f"{where}: expected a table")
            continue
        fields(err, where, save, required={"kind": str, "windows": str, "prefix": str},
               optional={"note": str})
        if save.get("kind") not in SAVE_KINDS:
            err(f"{where}.kind: expected one of {', '.join(sorted(SAVE_KINDS))}")
        if not re.match(r"^%[A-Za-z0-9_()]+%\\", save.get("windows", "")):
            err(f"{where}.windows: expected a path starting with a %VARIABLE%, "
                "so it can be resolved on whatever machine the saves came from")
        check_relative_path(err, f"{where}.prefix", save.get("prefix", ""),
                            starts_with="drive_c/", wildcard=True)

    status = data["status"]
    fields(err, "[status]", status, required={"state": str, "summary": str},
           optional={"stops-at": str, "blocked-by": str,
                     "last-verified": datetime.date, "verified-with": str})
    state = status.get("state")
    if state not in STATES:
        err(f"[status].state: '{state}' is not one of {', '.join(STATES)}")
    if state in ("menus", "broken") and not status.get("stops-at"):
        err("[status].stops-at: required unless the title works -- "
            "'does not launch' is not actionable, say where it stops")
    if state == "playable" and "stops-at" in status:
        err("[status].stops-at: a playable title does not stop anywhere")
    if (state == "broken") != ("blocked-by" in status):
        err("[status].blocked-by: required for a broken title, and only for one")
    if "blocked-by" in status and status["blocked-by"] not in BLOCKED_BY:
        err(f"[status].blocked-by: expected one of {', '.join(sorted(BLOCKED_BY))}")
    if state == "untested":
        if "last-verified" in status:
            err("[status].last-verified: an untested title has not been verified")
    elif "last-verified" not in status:
        err("[status].last-verified: required -- a matrix entry with no date is a rumour")
    elif isinstance(status["last-verified"], datetime.datetime):
        # A bare date and a date-time are different TOML types, and the latter
        # satisfies isinstance(..., datetime.date) while failing to compare.
        err("[status].last-verified: expected a bare date (YYYY-MM-DD), not a date-time")
    # A day of slack, because "today" is not one date everywhere. A recipe
    # written from a UTC+3 evening carries tomorrow's date as far as a CI runner
    # on UTC, and rejecting that reports a timezone as a mistake in the recipe.
    # Anything beyond a day is a typo worth catching.
    elif status["last-verified"] > datetime.date.today() + datetime.timedelta(days=1):
        err(f"[status].last-verified: {status['last-verified']} is in the future")
    if state == "playable-with-issues" and not data.get("issues"):
        err("[status].state: 'playable-with-issues' needs at least one [[issues]] entry")

    for i, issue in enumerate(data.get("issues", [])):
        where = f"[[issues]] #{i + 1}"
        if not isinstance(issue, dict):
            err(f"{where}: expected a table")
            continue
        fields(err, where, issue, required={"symptom": str, "cause": str, "fix": str},
               optional={"log-match": str})
        if "log-match" in issue:
            try:
                re.compile(issue["log-match"])
            except re.error as exc:
                err(f"{where}.log-match: not a valid regular expression ({exc})")

    return data, problems


def matrix(recipes):
    """The compatibility table, generated so it cannot drift from the recipes."""
    rank = list(STATES)
    rows = sorted(recipes, key=lambda r: (rank.index(r["status"]["state"]),
                                          r["title"]["name"]))
    out = ["| Title | Store id | State | Notes | Verified |",
           "|---|---|---|---|---|"]
    for r in rows:
        status = r["status"]
        note = status["summary"]
        if status.get("stops-at"):
            note += f" Stops at: {status['stops-at']}."
        out.append("| {} | `{}` | {} | {} | {} |".format(
            r["title"]["name"], r["title"]["product-id"], STATES[status["state"]],
            note, status.get("last-verified", "never")))
    return "\n".join(out)


def main(argv):
    want_matrix = "--matrix" in argv
    argv = [a for a in argv if a != "--matrix"]
    runtime_file = None
    if "--runtime" in argv:
        i = argv.index("--runtime")
        if i + 1 >= len(argv):
            print("!! --runtime needs a file", file=sys.stderr)
            return 2
        runtime_file = argv[i + 1]
        del argv[i:i + 2]

    # A recipe is named after its product id, so anything else in here is either
    # the registry or something that does not belong. Say so rather than
    # skipping it quietly, but do not fail the run over a stray file.
    strays = []
    if argv:
        paths = [Path(a) for a in argv]
    else:
        paths = []
        for candidate in sorted(HERE.glob("*.toml")):
            if candidate.name == "capabilities.toml":
                continue
            (paths if RECIPE_NAME.fullmatch(candidate.name) else strays).append(candidate)
    for stray in strays:
        print(f"** {stray.name} is not named after a Store product id, so it is not "
              "a recipe; see SCHEMA.md")
    if not paths:
        print("!! no recipes found", file=sys.stderr)
        return 2

    known, problems = load_capabilities(HERE / "capabilities.toml")
    failed = bool(problems)
    for problem in problems:
        print(f"!! {problem}", file=sys.stderr)

    provided = None
    if runtime_file:
        try:
            provided = load_runtime_capabilities(runtime_file)
        except OSError as exc:
            print(f"!! {exc}", file=sys.stderr)
            return 2
        for name in sorted(provided - set(known or {})):
            print(f"** {runtime_file} provides '{name}', which is not in capabilities.toml")

    recipes, seen = [], {}
    for path in paths:
        data, problems = check_recipe(path, known)
        if data is not None:
            for key in ("product-id", "slug"):
                value = data["title"].get(key)
                if value in seen.get(key, {}):
                    problems.append(f"[title].{key}: '{value}' is also used by "
                                    f"{seen[key][value]}")
                seen.setdefault(key, {})[value] = path.name
            if provided is not None:
                for name in data["runtime"].get("requires", []):
                    if name not in provided:
                        problems.append(f"[runtime].requires: the runtime does not "
                                        f"provide '{name}'")
            recipes.append(data)
        if problems:
            failed = True
            print(f"!! {path.name}", file=sys.stderr)
            for problem in problems:
                print(f"   {problem}", file=sys.stderr)

    if failed:
        return 1
    if want_matrix:
        print(matrix(recipes))
    else:
        print(f":: {len(recipes)} recipes, {len(known)} capabilities, no problems")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
