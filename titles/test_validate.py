#!/usr/bin/env python3
"""Tests for validate.py: the rules a recipe has to meet before it ships.

    python3 titles/test_validate.py
"""
import importlib.util
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("validate", HERE / "validate.py")
validate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validate)

MINIMAL = """\
schema = 1
{top}
[title]
product-id = "9NBLGGH2JHXJ"
name = {name}
slug = "{slug}"
{install}
[launch]
executable = 'Example.exe'
{launch}
[runtime]
requires = ["loader.memfd-main-image"]
[status]
state = "menus"
summary = {summary}
stops-at = "the title screen"
last-verified = 2026-01-01
"""


def check(top="", name='"Example"', install="", launch="", summary='"Starts."',
          slug="example"):
    known, problems = validate.load_capabilities(HERE / "capabilities.toml")
    assert not problems, problems
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "9NBLGGH2JHXJ.toml"
        path.write_text(MINIMAL.format(top=top, name=name, install=install,
                                       launch=launch, summary=summary, slug=slug))
        return validate.check_recipe(path, known)


class Recipes(unittest.TestCase):
    def test_the_minimal_recipe_is_clean(self):
        data, problems = check()
        self.assertEqual(problems, [])
        self.assertIsNotNone(data)

    def test_a_titles_own_directory_variables_are_accepted(self):
        for slug, dir_env, prefix_env in (("bedrock", "BEDROCK_DIR", "BEDROCK_PREFIX"),
                                          ("fh5", "FH5_DIR", "FH5_PREFIX"),
                                          ("halo-infinite", "HALO_INFINITE_DIR",
                                           "HALO_INFINITE_PREFIX")):
            _, problems = check(slug=slug, install=f'[install]\ndir-env = "{dir_env}"\n'
                                                   f'prefix-env = "{prefix_env}"\n')
            self.assertEqual(problems, [], slug)

    def test_the_launchers_own_variables_are_refused(self):
        for name in ("XODUS_GAMES_DIR", "XDG_RUNTIME_DIR", "HOME", "FERESTRE_TITLES",
                     "STEAM_COMPAT_DATA_PATH", "WINEPREFIX", "PROTON_DIR", "TMPDIR"):
            for key in ("dir-env", "prefix-env"):
                _, problems = check(install=f'[install]\n{key} = "{name}"\n')
                self.assertTrue(any(key in p for p in problems), (key, name, problems))

    def test_only_the_slugs_own_variable_is_accepted(self):
        for name in ("CARGO_TARGET_DIR", "PULSE_RUNTIME_DIR", "BEDROCK_DIR", "EXAMPLE_PREFIX"):
            _, problems = check(install=f'[install]\ndir-env = "{name}"\n')
            self.assertTrue(any("EXAMPLE_DIR" in p for p in problems), (name, problems))
        _, problems = check(install='[install]\nprefix-env = "EXAMPLE_DIR"\n')
        self.assertTrue(any("EXAMPLE_PREFIX" in p for p in problems), problems)
        _, problems = check(slug="xdg-runtime",
                            install='[install]\ndir-env = "XDG_RUNTIME_DIR"\n')
        self.assertTrue(any("dir-env" in p for p in problems), problems)

    def test_arguments_are_not_a_key_the_launcher_reads(self):
        _, problems = check(launch='arguments = ["-windowed"]')
        self.assertTrue(any("arguments" in p for p in problems), problems)

    def test_a_shipped_recipe_does_not_claim_to_be_generated(self):
        _, problems = check(top="generated = true")
        self.assertTrue(any("generated" in p for p in problems), problems)

    def test_a_matrix_cell_is_one_line(self):
        for field in ("name", "summary"):
            _, problems = check(**{field: '"""Two\nlines"""'})
            self.assertTrue(any("one line" in p for p in problems), (field, problems))


class Matrix(unittest.TestCase):
    def test_a_pipe_does_not_open_another_column(self):
        data, problems = check(name='"Left | Right"', summary='"A | B."')
        self.assertEqual(problems, [])
        row = validate.matrix([data]).splitlines()[2]
        self.assertIn(r"Left \| Right", row)
        self.assertIn(r"A \| B.", row)
        # Five columns: six unescaped bars.
        unescaped = row.replace(r"\|", "")
        self.assertEqual(unescaped.count("|"), 6, row)


if __name__ == "__main__":
    unittest.main()
