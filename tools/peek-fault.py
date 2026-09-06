#!/usr/bin/env python3
"""Dump instruction bytes from a running Wine process.

    peek-fault.py <launcher script> <exe name> <addr> [addr...]

The game's executable only exists decrypted inside a memfd, so bytes at a
faulting address cannot be read from the file on disk -- they have to come from
the live process. ptrace_scope is 1 on this machine, which lets a process read
only its own descendants, so this script starts the game itself rather than
attaching to one started elsewhere.
"""
import os


def games_dir():
    """Where the games live; override with XODUS_GAMES_DIR."""
    return os.environ.get("XODUS_GAMES_DIR", os.path.expanduser("~/xbox-games"))

import os
import re
import subprocess
import sys
import time


def find_pid(exe_name, deadline, seen):
    while time.time() < deadline:
        for pid in os.listdir("/proc"):
            if not pid.isdigit():
                continue
            try:
                with open(f"/proc/{pid}/cmdline", "rb") as f:
                    cmd = f.read().decode("utf-8", "replace")
            except OSError:
                continue
            if exe_name not in cmd or "peek-fault" in cmd:
                continue
            seen.setdefault(pid, cmd.replace("\x00", " ").strip()[:110])
            # skip the launcher chain: shell, xodus-cli, the proton script and
            # the steam.exe stub proton runs the title under
            if any(x in cmd for x in ("launch-", "xodus-cli", "/proton", "system32\\steam.exe")):
                continue
            return int(pid)
        time.sleep(0.5)
    return None


def read_mem(pid, addr, before=32, length=96):
    with open(f"/proc/{pid}/mem", "rb", 0) as m:
        m.seek(addr - before)
        return m.read(length)


def disassemble(data, base):
    path = "/tmp/peek-fault.bin"
    open(path, "wb").write(data)
    out = subprocess.run(
        ["objdump", "-D", "-b", "binary", "-m", "i386:x86-64",
         f"--adjust-vma={base:#x}", path],
        capture_output=True, text=True).stdout
    return "\n".join(out.splitlines()[7:])


def main():
    launcher, exe_name = sys.argv[1], sys.argv[2]
    addrs = [int(a, 16) for a in sys.argv[3:]]

    subprocess.run([os.path.join(games_dir(), "kill-gdk.sh")],
                   capture_output=True)

    proc = subprocess.Popen(["timeout", "120", launcher],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        seen = {}
        pid = find_pid(exe_name, time.time() + 60, seen)
        if not pid:
            print("no wine process for the image appeared; candidates seen:")
            for k, v in seen.items():
                print(f"  {k}: {v}")
            return 1
        print(f"game pid {pid}")

        # let it get past image load before reading
        time.sleep(3)
        for addr in addrs:
            print(f"\n=== {addr:#x} ===")
            try:
                data = read_mem(pid, addr)
            except OSError as e:
                print(f"  read failed: {e}")
                continue
            print(disassemble(data, addr - 32))
    finally:
        proc.terminate()
        subprocess.run([os.path.join(games_dir(), "kill-gdk.sh")], capture_output=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
