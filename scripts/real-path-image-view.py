#!/usr/bin/env python3
"""Launch with licensed descriptor images at their real, private file paths.

The normal Xodus client supplies WINE_DLL_FILE_MAP. Its plaintext stays in
memory: this helper copies it to tmpfs and binds it over the encrypted files
only in the launch's user/mount namespace. The package on disk is untouched.
"""

import os
import ctypes
import fcntl
from pathlib import Path
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import time

# How long the launch waits for a Wine session on its prefix that started
# outside the view; see wait_for_outside_server.
SERVER_WAIT = 10


def descendants(pid):
    try:
        children = [int(n) for n in Path(f"/proc/{pid}/task/{pid}/children").read_text().split()]
    except (FileNotFoundError, ProcessLookupError):
        return []
    return children + [child for parent in children for child in descendants(parent)]


def stop_descendants():
    """End only this launch's children before releasing its private mounts."""
    for signum, grace in ((signal.SIGTERM, 3), (signal.SIGKILL, 1)):
        for pid in descendants(os.getpid()):
            try:
                # A kernel process reference prevents a recycled PID from
                # receiving the signal if a child exits during cleanup.
                fd = os.pidfd_open(pid)
                try:
                    signal.pidfd_send_signal(fd, signum)
                finally:
                    os.close(fd)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline:
            try:
                while os.waitpid(-1, os.WNOHANG)[0]:
                    pass
            except ChildProcessError:
                return
            if not descendants(os.getpid()):
                return
            time.sleep(0.05)
    if descendants(os.getpid()):
        raise OSError("launch descendants did not exit")


def as_caller(uid, gid):
    """Return a preexec hook that gives the launch the caller's own ids back.

    Mounting needs root in this namespace, but Wine keys its server directory
    on getuid(): as uid 0 it would use /tmp/.wine-0, which another user can own
    or create first. A nested user namespace maps the caller's ids onto this
    one's root. It shares the mount namespace, so the private views stay in
    place, and the launch ends up with no capabilities over them.
    """
    def enter():
        if hasattr(os, "unshare"):
            os.unshare(os.CLONE_NEWUSER)
        else:
            libc = ctypes.CDLL(None, use_errno=True)
            if libc.unshare(0x10000000):  # CLONE_NEWUSER
                raise OSError(ctypes.get_errno(), "could not create the launch's user namespace")
        Path("/proc/self/setgroups").write_text("deny")
        Path("/proc/self/uid_map").write_text(f"{uid} 0 1")
        Path("/proc/self/gid_map").write_text(f"{gid} 0 1")
    return enter


def outside_server_lock(uid):
    """The lock file of a wineserver already serving this prefix, if any.

    Wine names the server directory after the prefix's device and inode, and
    the server holds a write lock on its lock file for as long as it runs.
    """
    prefix = os.environ.get("WINEPREFIX")
    if not prefix and os.environ.get("STEAM_COMPAT_DATA_PATH"):
        prefix = os.path.join(os.environ["STEAM_COMPAT_DATA_PATH"], "pfx")
    if not prefix:
        return None
    try:
        info = os.stat(prefix)
    except OSError:
        return None
    return Path(f"/tmp/.wine-{uid}/server-{info.st_dev:x}-{info.st_ino:x}/lock")


def server_running(lock):
    # Waiting is a courtesy to the registry; the private server directory is
    # what keeps the view safe, so a lock that cannot be read counts as free.
    try:
        fd = os.open(lock, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return False
    try:
        # struct flock: l_type, l_whence, l_start, l_len, l_pid. F_GETLK only
        # reports a conflicting lock; it does not register with the server
        # the way connecting to its socket would.
        probe = struct.pack("hhqqi4x", fcntl.F_WRLCK, os.SEEK_SET, 0, 1, 0)
        held = struct.unpack("hhqqi4x", fcntl.fcntl(fd, fcntl.F_GETLK, probe))
        return held[0] != fcntl.F_UNLCK
    finally:
        os.close(fd)


def wait_for_outside_server(uid, limit=SERVER_WAIT):
    """Let a session started outside the view end before the launch.

    The launch gets a server of its own (see private_server_dir), and two
    servers on one prefix each save the registry when they exit, the later
    one over the earlier. The shim's own `proton run reg add` leaves such a
    session behind for a few seconds on a prefix's first launch. One that
    outlasts the limit is another title on the prefix: the launch goes ahead
    beside it rather than wait for it to be closed.
    """
    lock = outside_server_lock(uid)
    if lock is None:
        return
    deadline = time.monotonic() + limit
    while server_running(lock):
        if time.monotonic() >= deadline:
            print("ferestre: a Wine session for this prefix is still running outside the "
                  "private image view; registry changes made by one of them may be lost",
                  file=sys.stderr)
            return
        time.sleep(0.1)


def private_server_dir(uid):
    """Give the launch a Wine server directory that only this view can see.

    Wine's client sends the server the Unix path of each file it opens, and
    the server opens it. A server started outside this mount namespace would
    open the encrypted package files, not the private images bound over them,
    so the launch must never join one. A tmpfs over /tmp/.wine-<uid> hides
    every outside server: the launch starts its own, here, which sees the
    view. The tmpfs is owned by this namespace's root, which is the caller in
    the launch's namespace, so Wine accepts it as its own.
    """
    base = Path(f"/tmp/.wine-{uid}")
    try:
        base.mkdir(mode=0o700)
    except FileExistsError:
        pass
    info = os.lstat(base)
    # Owned by the caller (root here), a directory in sticky /tmp cannot be
    # swapped for a link by anyone else between this check and the mount.
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        raise ValueError(f"{base} is not a directory owned by this user")
    subprocess.run(["mount", "-t", "tmpfs", "-o", "mode=0700,nodev,nosuid",
                    "tmpfs", str(base)], check=True)
    return base


def image_entries(mapping, root):
    if not mapping:
        raise ValueError("the client supplied no decrypted image descriptors")
    entries = []
    seen = set()
    for item in mapping.split("|"):
        descriptor, nt_path = item.split(":", 1)
        descriptor = int(descriptor)
        if descriptor < 3 or not nt_path.startswith("\\??\\Z:\\"):
            raise ValueError("invalid mapped image descriptor or path")
        target = Path("/" + nt_path[7:].replace("\\", "/")).resolve(strict=True)
        if not target.is_relative_to(root) or not target.is_file() or target in seen:
            raise ValueError("mapped image must be a distinct file inside the install directory")
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or os.pread(descriptor, 2, 0) != b"MZ":
            raise ValueError("mapped image descriptor does not contain a PE image")
        seen.add(target)
        entries.append((descriptor, target, info.st_size))
    return entries


def main():
    args = sys.argv[1:]
    inside = args[:1] == ["--inside"]
    if inside:
        parent_mount = args[1]
        uid, gid = int(args[2]), int(args[3])
        args = args[4:]
        if os.readlink("/proc/self/ns/mnt") == parent_mount:
            raise ValueError("refusing to mount images outside a private namespace")
    if args[:1] == ["--"]:
        args = args[1:]
    if not args:
        raise ValueError("no launch command supplied")
    entries = image_entries(os.environ.get("WINE_DLL_FILE_MAP"), Path.cwd().resolve())
    if not inside:
        os.execvp("unshare", [
            "unshare", "--user", "--map-root-user", "--mount",
            "--propagation", "private", sys.executable, str(Path(__file__).resolve()),
            "--inside", os.readlink("/proc/self/ns/mnt"), str(os.getuid()), str(os.getgid()),
            "--", *args,
        ])

    targets = []
    signal.signal(signal.SIGTERM, signal.default_int_handler)
    # Wine can detach its processes from Proton. Adopt those orphans so Stop
    # and error cleanup can still reach them without affecting another prefix.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0):  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "could not supervise launch descendants")
    with tempfile.TemporaryDirectory(prefix="ferestre-images-") as directory:
        stage = Path(directory)
        mounted = False
        server_dir = None
        try:
            size = sum((n + 4095) // 4096 * 4096 for _, _, n in entries) + 8 * 1024 * 1024
            subprocess.run(["mount", "-t", "tmpfs", "-o",
                            f"mode=0700,nodev,nosuid,size={size}", "tmpfs", str(stage)], check=True)
            mounted = True
            for index, (descriptor, target, _) in enumerate(entries):
                source = stage / str(index)
                # Reopen rather than duplicate: do not change the client's
                # descriptor offset, even when an earlier read consumed it.
                with open(f"/proc/self/fd/{descriptor}", "rb") as src, source.open("wb") as dst:
                    shutil.copyfileobj(src, dst)
                # Preserve Windows-visible attributes. A chmod(0400) image
                # gains FILE_ATTRIBUTE_READONLY even though the package file
                # did not have it; the read-only bind already protects bytes.
                info = target.stat()
                source.chmod(stat.S_IMODE(info.st_mode))
                os.utime(source, ns=(info.st_atime_ns, info.st_mtime_ns))
                subprocess.run(["mount", "--bind", str(source), str(target)], check=True)
                targets.append(target)
                subprocess.run(["mount", "-o", "remount,bind,ro", str(target)], check=True)
            wait_for_outside_server(uid)
            server_dir = private_server_dir(uid)
            env = dict(os.environ)
            env.pop("WINE_DLL_FILE_MAP", None)
            env.pop("FERESTRE_IMAGE_VIEW", None)
            result = subprocess.call(args, env=env, close_fds=True, preexec_fn=as_caller(uid, gid))
            if result == 0:
                # The bootstrapper may exit after handing off to another game
                # image (Halo's campaign does this). Keep both the namespace
                # and the licensed images until the adopted children finish.
                while True:
                    try:
                        os.waitpid(-1, 0)
                    except ChildProcessError:
                        break
            return result
        finally:
            try:
                stop_descendants()
            finally:
                if server_dir:
                    subprocess.run(["umount", "-l", str(server_dir)], check=False)
                for target in reversed(targets):
                    subprocess.run(["umount", "-l", str(target)], check=False)
                if mounted:
                    subprocess.run(["umount", "-l", str(stage)], check=False)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"ferestre: could not prepare the private image view: {error}", file=sys.stderr)
        sys.exit(1)
