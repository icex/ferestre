#!/usr/bin/env python3
"""Characterise a memfd-only game image: entry point, TLS callbacks, load
config, and which sections are writable+executable (runtime-decrypt targets).

    pe-inspect.py <launcher> <exe name>

Purpose here is diagnosis, not modification: does the title rely on an image
init step (a TLS callback, a load-config SEH/CFG table) that our memfd loader
path might be skipping -- a legitimate compatibility gap -- or does it fill its
own code in at run time behind a protection stub. The two need different
answers, and this tells them apart without touching the image.

Reads from the xodus-cli memfd of a launch it starts itself (ptrace_scope=1
allows only a descendant's descriptors).
"""
import os


def games_dir():
    """Where the games live; override with XODUS_GAMES_DIR."""
    return os.environ.get("XODUS_GAMES_DIR", os.path.expanduser("~/xbox-games"))

import os
import struct
import subprocess
import sys
import time

DIR_NAMES = {0: "EXPORT", 1: "IMPORT", 3: "EXCEPTION", 5: "BASERELOC",
             6: "DEBUG", 9: "TLS", 10: "LOAD_CONFIG"}


def find_memfd(deadline):
    while time.time() < deadline:
        for pid in os.listdir("/proc"):
            if not pid.isdigit():
                continue
            try:
                with open(f"/proc/{pid}/cmdline", "rb") as f:
                    cmd = f.read().decode("utf-8", "replace")
            except OSError:
                continue
            if "xodus-cli" not in cmd or "pe-inspect" in cmd:
                continue
            try:
                for fd in os.listdir(f"/proc/{pid}/fd"):
                    if "memfd:" in os.readlink(f"/proc/{pid}/fd/{fd}"):
                        return int(pid), int(fd)
            except OSError:
                continue
        time.sleep(0.3)
    return None, None


def main():
    launcher, exe_name = sys.argv[1], sys.argv[2]
    subprocess.run([os.path.join(games_dir(), "kill-gdk.sh")], capture_output=True)
    proc = subprocess.Popen(["timeout", "90", launcher],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        pid, fd = find_memfd(time.time() + 45)
        if not pid:
            print("no memfd appeared")
            return 1
        path = f"/proc/{pid}/fd/{fd}"

        def read_at(off, length):
            with open(path, "rb", 0) as f:
                f.seek(off)
                return f.read(length)

        # wait until the header and section table are present
        header = None
        for _ in range(180):
            try:
                if os.stat(path).st_size >= 0x1000:
                    header = read_at(0, 0x1000)
                    break
            except OSError:
                break
            time.sleep(0.3)
        if not header or header[:2] != b"MZ":
            print("no PE header")
            return 1

        pe = struct.unpack_from("<I", header, 0x3c)[0]
        magic = struct.unpack_from("<H", header, pe + 24)[0]
        entry = struct.unpack_from("<I", header, pe + 24 + 16)[0]
        base = struct.unpack_from("<Q", header, pe + 24 + 24)[0]
        nsec = struct.unpack_from("<H", header, pe + 6)[0]
        optsz = struct.unpack_from("<H", header, pe + 20)[0]
        dllchar = struct.unpack_from("<H", header, pe + 24 + 70)[0]
        ndirs = struct.unpack_from("<I", header, pe + 24 + 108)[0]
        dd = pe + 24 + 112
        dirs = {i: struct.unpack_from("<II", header, dd + i * 8) for i in range(min(ndirs, 16))}

        secs = []
        for i in range(nsec):
            o = pe + 24 + optsz + i * 40
            name = header[o:o + 8].rstrip(b"\0").decode(errors="replace")
            vsz, va, rsz, ptr = struct.unpack_from("<IIII", header, o + 8)
            chars = struct.unpack_from("<I", header, o + 36)[0]
            secs.append((name, va, vsz, ptr, rsz, chars))

        def rva2off(rva):
            for name, va, vsz, ptr, rsz, chars in secs:
                if va <= rva < va + max(vsz, rsz):
                    return ptr + (rva - va)
            return None

        def wait_off(off, n=64):
            for _ in range(180):
                try:
                    if os.stat(path).st_size >= off + n:
                        return read_at(off, n)
                except OSError:
                    return None
                time.sleep(0.3)
            return None

        print(f"image base {base:#x}, entry RVA {entry:#x} "
              f"({'in ' + next((s[0] for s in secs if s[1] <= entry < s[1]+max(s[2],s[4])), '?')})")
        cfg = " ".join(n for bit, n in [(0x40, "DYNAMIC_BASE/ASLR"), (0x100, "NX"),
                                        (0x400, "NO_SEH"), (0x4000, "CFG")] if dllchar & bit)
        print(f"DllCharacteristics: {dllchar:#06x}  [{cfg}]")

        print("data directories present:")
        for i, (rva, sz) in dirs.items():
            if rva and i in DIR_NAMES:
                print(f"  {DIR_NAMES[i]:<12} rva {rva:#x} size {sz:#x}")

        # TLS callbacks: dir[9] -> IMAGE_TLS_DIRECTORY; AddressOfCallBacks (VA)
        tls_rva = dirs.get(9, (0, 0))[0]
        if tls_rva:
            off = rva2off(tls_rva)
            tls = wait_off(off, 40) if off is not None else None
            if tls:
                cb_va = struct.unpack_from("<Q", tls, 24)[0]
                print(f"TLS directory at rva {tls_rva:#x}; AddressOfCallBacks VA {cb_va:#x}")
                if cb_va:
                    cb_off = rva2off(cb_va - base)
                    arr = wait_off(cb_off, 8 * 32) if cb_off is not None else None
                    n = 0
                    if arr:
                        for k in range(32):
                            v = struct.unpack_from("<Q", arr, k * 8)[0]
                            if not v:
                                break
                            print(f"  TLS callback {k}: VA {v:#x} (rva {v-base:#x})")
                            n += 1
                    print(f"  -> {n} TLS callback(s)")
                else:
                    print("  no TLS callbacks")
        else:
            print("no TLS directory")

        we = [s for s in secs if (s[5] & 0x80000000) and (s[5] & 0x20000000)]
        print(f"writable+executable sections (runtime-decrypt candidates): "
              f"{[s[0] for s in we] or 'none'}")
    finally:
        proc.terminate()
        subprocess.run([os.path.join(games_dir(), "kill-gdk.sh")], capture_output=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
