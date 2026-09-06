#!/usr/bin/env python3
"""Put the compatibility matrix into docs/COMPATIBILITY.md, and keep it there.

The table is rendered by `titles/validate.py --matrix`, which is also what
validates the recipes: the schema has one owner and the page has one. This
script decides only where the table goes in the document and whether the
committed document still matches the recipes.

    .github/scripts/gen-compatibility.py            rewrite the generated block
    .github/scripts/gen-compatibility.py --check    fail if it is out of date

The published matrix is the page people read before deciding whether this
project is worth their evening, so it must not be able to disagree with the
recipes the launcher runs from. A hand-maintained table goes stale the first
time a title changes state and nobody remembers the second place it was written
down; CI runs --check so that fails the build instead of quietly misleading
someone.

Because the validator is what produces the table, a recipe that does not
validate stops this too, with the validator's own message.
"""

from __future__ import annotations

import argparse
import difflib
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

BEGIN = "<!-- BEGIN GENERATED MATRIX (.github/scripts/gen-compatibility.py) -->"
END = "<!-- END GENERATED MATRIX -->"


def die(message: str) -> None:
    print(f"gen-compatibility: {message}", file=sys.stderr)
    raise SystemExit(2)


def render_table(validator: Path) -> str:
    if not validator.is_file():
        die(f"{validator} does not exist")
    result = subprocess.run([sys.executable, str(validator), "--matrix"],
                            capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        die(f"{validator.name} rejected the recipes; fix them first")

    # The validator also prints notes about stray files on stdout, so take the
    # table itself rather than everything it said.
    lines = result.stdout.splitlines()
    start = next((i for i, line in enumerate(lines) if line.startswith("| Title |")), None)
    if start is None:
        die(f"{validator.name} --matrix printed no table:\n{result.stdout}")
    table = [line for line in lines[start:] if line.startswith("|")]
    return "\n".join(table)


def render_block(validator: Path) -> str:
    return "\n".join([
        BEGIN,
        "",
        render_table(validator),
        "",
        f"Generated from `titles/*.toml`. Each row's recipe is the file named after"
        f" its Store id, and `titles/SCHEMA.md` says what is in one.",
        "",
        END,
    ])


def splice(doc: str, block: str, doc_path: Path) -> str:
    start = doc.find(BEGIN)
    end = doc.find(END)
    if start < 0 or end < 0 or end < start:
        die(f"{doc_path}: missing the generated-block markers.\n"
            f"  Add these two lines where the matrix belongs:\n"
            f"    {BEGIN}\n    {END}")
    return doc[:start] + block + doc[end + len(END):]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true",
                        help="exit non-zero if the committed page is out of date")
    parser.add_argument("--validator", type=Path, default=REPO / "titles" / "validate.py",
                        help="the recipe validator that renders the table")
    parser.add_argument("--doc", type=Path, default=REPO / "docs" / "COMPATIBILITY.md",
                        help="the page whose generated block is rewritten")
    args = parser.parse_args()

    if not args.doc.is_file():
        die(f"{args.doc} does not exist")
    current = args.doc.read_text(encoding="utf-8")
    updated = splice(current, render_block(args.validator), args.doc)

    if args.check:
        if current == updated:
            print(f"gen-compatibility: {args.doc.name} matches the recipes")
            return 0
        sys.stdout.writelines(difflib.unified_diff(
            current.splitlines(True), updated.splitlines(True),
            fromfile=f"{args.doc} (committed)", tofile=f"{args.doc} (generated)"))
        print("\ngen-compatibility: out of date. Run "
              ".github/scripts/gen-compatibility.py and commit the result.",
              file=sys.stderr)
        return 1

    if current == updated:
        print(f"gen-compatibility: {args.doc.name} already current")
        return 0
    args.doc.write_text(updated, encoding="utf-8")
    print(f"gen-compatibility: wrote {args.doc}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
