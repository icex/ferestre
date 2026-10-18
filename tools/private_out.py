"""The output directory for the desktop drivers in tools/, kept private.

auto-join.py, drive-aoe.py and drive-fh5.py write things into their output
directory that nobody else may change or read: input scripts the Wine bot
carries out, a KWin script the compositor runs, launch logs that name the
account, and screenshots of the whole desktop. A fixed name under /tmp is a
directory any other local user can create first, and whoever owns a directory
can swap the files in it between a write and the read that follows. So the
directory has to belong to this user and be closed to writes from anyone else,
and no directory above it may be one that someone else can rename it out of.
"""

import os
from pathlib import Path
import stat


def default_root():
    """Where an output directory goes when none is named.

    The session's runtime directory is this user's alone and is emptied at
    logout, which suits screenshots and log tails. Without one, the cache
    directory in the home directory is the next private place.
    """
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime and os.path.isabs(runtime):
        return Path(runtime)
    cache = os.environ.get("XDG_CACHE_HOME")
    if cache and os.path.isabs(cache):
        return Path(cache)
    return Path.home() / ".cache"


def refuse(path, why):
    raise SystemExit(f"!! refusing to use {path} for output: {why}")


def check_owned_and_closed(path, uid):
    info = os.lstat(path)
    if stat.S_ISLNK(info.st_mode):
        refuse(path, "it is a symbolic link")
    if not stat.S_ISDIR(info.st_mode):
        refuse(path, "it is not a directory")
    if info.st_uid != uid:
        refuse(path, f"it belongs to uid {info.st_uid}, not to this user")
    if info.st_mode & 0o022:
        refuse(path, f"other users can write to it (mode {stat.S_IMODE(info.st_mode):o})")


def check_ancestors(path, uid):
    """Every directory above `path` must keep strangers from renaming it.

    A directory another user can write to lets them move ours aside and put
    their own in its place, unless it is sticky like /tmp, where only the owner
    of an entry may rename it.
    """
    for parent in path.parents:
        info = os.stat(parent)
        if info.st_uid not in (0, uid):
            refuse(path, f"{parent} belongs to uid {info.st_uid}")
        if info.st_mode & 0o022 and not info.st_mode & stat.S_ISVTX:
            refuse(path, f"other users can write to {parent}")


def private_dir(override, name):
    """The output directory: `override` when set, else `name` under the
    default root. Created mode 0700 when missing; refused, with a message and a
    non-zero exit, when it is not this user's or others can write to it."""
    path = Path(override) if override else default_root() / name
    path = Path(os.path.abspath(path))
    try:
        path.mkdir(parents=True, mode=0o700)
    except FileExistsError:
        pass
    # Resolve once and use the resolved path from here on: a symbolic link
    # that someone else can point elsewhere is then out of the picture.
    real = Path(os.path.realpath(path))
    uid = os.geteuid()
    check_owned_and_closed(real, uid)
    check_ancestors(real, uid)
    return real
