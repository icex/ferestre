#!/usr/bin/env python3
"""Find elided call sites in a PE image.

    pe-holes.py <file.exe|file.dll> [--sample N] [--verbose]

Some Store titles ship with their direct `call`/`jmp rel32` instructions
removed: the five bytes are replaced with filler whose first byte is an opcode
that does not exist in 64-bit mode, so executing one faults immediately with
STATUS_ILLEGAL_INSTRUCTION instead of running somewhere arbitrary. A runtime
component is expected to write the real bytes back before the code is reached.
When that component does not run, the title dies calling one of them, and the
symptom -- an illegal instruction inside the game with a perfectly ordinary
backtrace -- looks exactly like a corrupt download or a botched decrypt.

It is neither, and telling them apart is what this is for. The test is
structural rather than statistical:

  * walk every function .pdata declares, decoding from its entry;
  * where decoding breaks, assume five bytes are missing and resume after them;
  * a function is *explained* if that rule carries the decode to the declared
    end with no bytes left over.

Damage does not do that. Random bytes in a random place leave a function that
cannot be made to decode by skipping a fixed five, and a wrong key garbles a
whole 4096-byte unit rather than landing on instruction boundaries. So a title
where nearly every function is explained, and where the filler's first byte is
drawn evenly from the opcodes that fault in long mode, is telling you its call
sites were taken out on purpose.

Run it against an ordinary DLL first: an untouched binary reports no holes at
all, which is what makes a positive result mean something.

Needs objdump (binutils). Reads an image straight from disk; for a title whose
executable is still encrypted on disk, capture the decrypted bytes first --
see docs/RECIPES.md.
"""

import argparse
import collections
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

# One-byte opcodes with no encoding in 64-bit mode. Executing one raises #UD,
# which is the property the filler is chosen for. 0xc4/0xc5/0x62 are left out
# on purpose -- they are VEX/EVEX prefixes here, not invalid.
NO_LONG_MODE = {0x06, 0x07, 0x0e, 0x16, 0x17, 0x1e, 0x1f, 0x27, 0x2f, 0x37,
                0x3f, 0x60, 0x61, 0x82, 0x9a, 0xce, 0xd4, 0xd5, 0xd6, 0xea}

INSN = re.compile(r"^\s+([0-9a-f]+):\t([0-9a-f ]+?)\s*\t(.*)$")


class Image:
    def __init__(self, path):
        self.data = Path(path).read_bytes()
        d = self.data
        if d[:2] != b"MZ":
            raise SystemExit(f"{path}: not a PE (no MZ). If the title ships it "
                             f"encrypted, capture the decrypted image first.")
        pe = struct.unpack_from("<I", d, 0x3c)[0]
        if d[pe:pe + 4] != b"PE\0\0":
            raise SystemExit(f"{path}: no PE header")
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        optsz = struct.unpack_from("<H", d, pe + 20)[0]
        opt = pe + 24
        if struct.unpack_from("<H", d, opt)[0] != 0x20b:
            raise SystemExit(f"{path}: not PE32+ (x86-64 only)")
        ndir = struct.unpack_from("<I", d, opt + 108)[0]
        self.pdata = struct.unpack_from("<II", d, opt + 112 + 8 * 3) if ndir > 3 else (0, 0)
        self.secs = []
        for i in range(nsec):
            o = opt + optsz + 40 * i
            name = d[o:o + 8].rstrip(b"\0").decode("ascii", "replace")
            vsz, va, rsz, ptr = struct.unpack_from("<IIII", d, o + 8)
            self.secs.append((name, va, vsz, ptr, rsz))

    def off(self, rva):
        """File offset for an RVA. Not the identity: FileAlignment is often
        0x200 while SectionAlignment is 0x1000, so .text at RVA 0x1000 can
        live at raw 0x600, and reading an image as if RVA were the offset
        silently analyses the wrong bytes."""
        for _, va, vsz, ptr, rsz in self.secs:
            if va <= rva < va + max(vsz, rsz):
                d = rva - va
                return ptr + d if d < rsz else None
        return None

    def functions(self):
        rva, size = self.pdata
        base = self.off(rva)
        if not base or not size:
            return []
        out = []
        for i in range(size // 12):
            b, e, _ = struct.unpack_from("<III", self.data, base + 12 * i)
            if e > b and self.off(b) is not None and self.off(e - 1) is not None:
                out.append((b, e))
        return out


def disassemble(path, start, stop):
    out = subprocess.run(
        ["objdump", "-D", "-b", "binary", "-m", "i386:x86-64", "-M", "intel",
         f"--start-address={start}", f"--stop-address={stop}", str(path)],
        capture_output=True).stdout.decode("utf8", "replace")
    return [(int(m.group(1), 16), m.group(2).strip(), m.group(3).strip())
            for m in (INSN.match(l) for l in out.splitlines()) if m]


def holes(path, img, b, e, cap=400):
    """Hole offsets in [b,e), or None if five-byte holes do not explain it."""
    cur, stop = img.off(b), img.off(e)
    found = []
    while cur < stop:
        for addr, byts, txt in disassemble(path, cur, stop):
            if addr != cur or "(bad)" in txt or txt.startswith("udb"):
                break
            cur += len(byts.split())
        if cur >= stop:
            break
        if len(found) >= cap:
            return None
        found.append(cur)
        cur += 5
    return found if cur == stop else None


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("image")
    ap.add_argument("--sample", type=int, default=300,
                    help="functions to examine (default 300; 0 means all)")
    ap.add_argument("--verbose", action="store_true", help="list the holes found")
    a = ap.parse_args()

    img = Image(a.image)
    fns = img.functions()
    if not fns:
        raise SystemExit(f"{a.image}: no .pdata, so there are no function bounds "
                         f"to check against")
    sample = fns if a.sample == 0 else fns[:a.sample]
    print(f":: {a.image}")
    print(f"   {len(fns):,} functions in .pdata, examining {len(sample):,}")

    explained = unexplained = untouched = 0
    first = collections.Counter()
    listed = []
    for b, e in sample:
        hs = holes(a.image, img, b, e)
        if hs is None:
            unexplained += 1
            continue
        explained += 1
        if not hs:
            untouched += 1
        for h in hs:
            first[img.data[h]] += 1
            if a.verbose and len(listed) < 40:
                listed.append((b, h, img.data[h:h + 5]))

    tot = sum(first.values())
    inv = sum(v for k, v in first.items() if k in NO_LONG_MODE)
    print(f"   explained by 5-byte holes  {explained:,}")
    print(f"   not explained              {unexplained:,}")
    print(f"   untouched (no holes)       {untouched:,}")
    print(f"   holes                      {tot:,}")
    if tot:
        pct = 100 * inv / tot
        print(f"   filler starting with an opcode invalid in 64-bit mode: "
              f"{inv:,} ({pct:.1f}%, chance {100*len(NO_LONG_MODE)/256:.1f}%)")
        print("   commonest filler first bytes: "
              + ", ".join(f"{k:#04x}x{v}" for k, v in first.most_common(8)))
    for b, h, byts in listed:
        print(f"     function {b:#x}: hole at file {h:#x}  {byts.hex(' ')}")

    if tot == 0:
        print("\n   No holes. This image's call sites are intact.")
    elif pct > 40:
        print("\n   Call sites have been elided deliberately: the filler's first"
              "\n   byte is drawn from the opcodes that fault in long mode, which"
              "\n   damage and a wrong decryption key do not do. Something is"
              "\n   expected to write the real bytes back at runtime.")
    else:
        print("\n   Holes found, but the filler does not look chosen. Suspect the"
              "\n   decode rather than the image, and check a known-good DLL too.")


if __name__ == "__main__":
    main()
