#!/usr/bin/env python3
"""Launch with licensed descriptor images at their real, private file paths.

The normal Xodus client supplies WINE_DLL_FILE_MAP. Its plaintext stays in
memory: this helper copies it to tmpfs and binds it over the encrypted files
only in the launch's user/mount namespace. The package on disk is untouched.
"""

import os
import ctypes
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time


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
        args = args[2:]
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
            "--inside", os.readlink("/proc/self/ns/mnt"), "--", *args,
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
            env = dict(os.environ)
            env.pop("WINE_DLL_FILE_MAP", None)
            env.pop("FERESTRE_IMAGE_VIEW", None)
            result = subprocess.call(args, env=env, close_fds=True)
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
                for target in reversed(targets):
                    subprocess.run(["umount", "-l", str(target)], check=False)
                if mounted:
                    subprocess.run(["umount", "-l", str(stage)], check=False)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"ferestre: could not prepare the private image view: {error}", file=sys.stderr)
        sys.exit(1)
