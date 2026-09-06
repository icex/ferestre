#!/usr/bin/env python3
"""Inspect the decrypted game image while a launch is in flight.

    peek-image.py <launcher> <exe name> [rva ...]

`xodus-cli` decrypts each protected executable into a memfd and passes the
descriptor to Wine, so the bytes never exist on disk -- the file there stays
ciphertext. The descriptor is, however, an ordinary open file in the xodus-cli
process, and that process is a direct descendant of this one, which is what
makes it readable under ptrace_scope=1.

Reports the PE section layout (protector stubs usually announce themselves
there) and disassembles around any RVA given.
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

WINDOW = 512


def find_xodus(deadline):
    """The xodus-cli process holding the memfds, and the memfd fd number."""
    while time.time() < deadline:
        for pid in os.listdir("/proc"):
            if not pid.isdigit():
                continue
            try:
                with open(f"/proc/{pid}/cmdline", "rb") as f:
                    cmd = f.read().decode("utf-8", "replace")
            except OSError:
                continue
            if "xodus-cli" not in cmd or "peek-image" in cmd:
                continue
            fds = {}
            try:
                for fd in os.listdir(f"/proc/{pid}/fd"):
                    try:
                        target = os.readlink(f"/proc/{pid}/fd/{fd}")
                    except OSError:
                        continue
                    if "memfd:" in target:
                        fds[int(fd)] = target
            except OSError:
                continue
            if fds:
                return int(pid), fds
        time.sleep(0.3)
    return None, {}


def read_at(pid, fd, offset, length):
    with open(f"/proc/{pid}/fd/{fd}", "rb", 0) as f:
        f.seek(offset)
        return f.read(length)


def parse_pe(header):
    pe = struct.unpack_from("<I", header, 0x3c)[0]
    nsec = struct.unpack_from("<H", header, pe + 6)[0]
    optsz = struct.unpack_from("<H", header, pe + 20)[0]
    base = struct.unpack_from("<Q", header, pe + 24 + 24)[0]
    secs = []
    for i in range(nsec):
        o = pe + 24 + optsz + i * 40
        name = header[o:o + 8].rstrip(b"\0").decode(errors="replace")
        vsz, va, rsz, ptr = struct.unpack_from("<IIII", header, o + 8)
        chars = struct.unpack_from("<I", header, o + 36)[0]
        secs.append((name, va, vsz, ptr, rsz, chars))
    return base, secs


def main():
    launcher, exe_name = sys.argv[1], sys.argv[2]
    rvas = [int(a, 16) for a in sys.argv[3:]]

    subprocess.run([os.path.join(games_dir(), "kill-gdk.sh")], capture_output=True)
    proc = subprocess.Popen(["timeout", "90", launcher],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        pid, fds = find_xodus(time.time() + 45)
        if not pid:
            print("xodus-cli with memfds never appeared")
            return 1
        print(f"xodus-cli pid {pid}, memfds: {sorted(fds)}")

        # the image for this executable is the largest memfd
        best_fd, best_size = None, -1
        for fd in fds:
            try:
                size = os.stat(f"/proc/{pid}/fd/{fd}").st_size
            except OSError:
                continue
            if size > best_size:
                best_fd, best_size = fd, size
        print(f"using fd {best_fd}, {best_size} bytes ({best_size/1024/1024:.2f} MB)")

        # xodus-cli fills the descriptor as it decrypts, and closes it when the
        # title exits -- which for a crashing title is soon. So rather than wait
        # for the whole image, grab each window the moment it is covered.
        header = None
        windows = {}
        deadline = time.time() + 90
        while time.time() < deadline:
            try:
                size = os.stat(f"/proc/{pid}/fd/{best_fd}").st_size
            except OSError:
                break
            try:
                if header is None and size >= 0x1000:
                    header = read_at(pid, best_fd, 0, 0x1000)
                    if header[:2] != b"MZ":
                        print("not a PE image")
                        return 1
                    base, secs = parse_pe(header)
                    print(f"image base {base:#x}, {len(secs)} sections, "
                          f"file is {max(x[3] + x[4] for x in secs)} bytes")
                    wanted = {}
                    for rva in rvas:
                        sec = next((x for x in secs
                                    if x[1] <= rva < x[1] + max(x[2], x[4])), None)
                        if sec:
                            wanted[rva] = (sec[3] + (rva - sec[1]), sec[0], sec[5])
                        else:
                            print(f"RVA {rva:#x} is not inside any section")
                if header is not None:
                    for rva, (off, secname, chars) in list(wanted.items()):
                        if rva in windows or size < off + WINDOW:
                            continue
                        windows[rva] = (read_at(pid, best_fd, off - 64, WINDOW), secname, chars)
                        print(f"captured RVA {rva:#x} from {secname}")
                    if wanted and len(windows) == len(wanted):
                        break
            except OSError:
                break
            time.sleep(0.2)

        for rva, (data, secname, chars) in windows.items():
            print(f"\n=== RVA {rva:#x} ({secname}, "
                  f"{'executable' if chars & 0x20000000 else 'NOT executable'}) ===")
            print("hex (starts 64 bytes before the RVA):")
            for i in range(0, len(data), 16):
                mark = "  <<<" if i == 64 else ""
                print(f"  {base + rva - 64 + i:#x}  {data[i:i+16].hex(' ')}{mark}")
            open("/tmp/peek-image.bin", "wb").write(data)
            out = subprocess.run(
                ["objdump", "-D", "-b", "binary", "-m", "i386:x86-64",
                 f"--adjust-vma={base + rva - 64:#x}", "/tmp/peek-image.bin"],
                capture_output=True, text=True).stdout
            print("\n".join(out.splitlines()[7:]))
        if rvas and not windows:
            print("could not capture any window before the descriptor closed")
    finally:
        proc.terminate()
        subprocess.run([os.path.join(games_dir(), "kill-gdk.sh")], capture_output=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
