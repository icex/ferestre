#!/usr/bin/env python3
"""Dump a Windows Gaming Services (WGS) save folder: containers.index, each
container's manifest, and the blob files behind it.

Usage: wgs_dump.py <path to <XUID>_<SCID> folder>

Independent of the Wine implementation; used to check what it writes.
"""
import struct
import sys
import uuid
from pathlib import Path


class Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def u8(self):
        (v,) = struct.unpack_from("<B", self.data, self.pos)
        self.pos += 1
        return v

    def u32(self):
        (v,) = struct.unpack_from("<I", self.data, self.pos)
        self.pos += 4
        return v

    def u64(self):
        (v,) = struct.unpack_from("<Q", self.data, self.pos)
        self.pos += 8
        return v

    def wstr(self):
        n = self.u32()
        s = self.data[self.pos:self.pos + 2 * n].decode("utf-16-le")
        self.pos += 2 * n
        return s

    def guid(self):
        g = uuid.UUID(bytes_le=self.data[self.pos:self.pos + 16])
        self.pos += 16
        return g


def filetime_to_str(ft):
    import datetime
    if ft < 116444736000000000:
        return str(ft)
    return datetime.datetime.utcfromtimestamp((ft - 116444736000000000) / 1e7).isoformat()


def blob_hash(data):
    h = 0
    for b in data:
        h = (h * 31 + b) & 0xFFFFFFFF
    return h


def main(folder):
    folder = Path(folder)
    r = Reader((folder / "containers.index").read_bytes())
    version, count, res0 = r.u32(), r.u32(), r.u32()
    aumid = r.wstr()
    mtime = r.u64()
    res1 = r.u32()
    ident = r.wstr()
    quota = r.u64()
    print(f"index v{version} count={count} res0={res0} res1={res1} quota={quota}")
    print(f"  aumid={aumid!r} id={ident!r} mtime={filetime_to_str(mtime)}")
    total = 0
    for _ in range(count):
        name, display, etag = r.wstr(), r.wstr(), r.wstr()
        number, flags = r.u8(), r.u32()
        guid = r.guid()
        cmtime, res, size = r.u64(), r.u64(), r.u64()
        total += size
        fold = guid.hex.upper()
        print(f"container {name!r} display={display!r} etag={etag} n={number} flags={flags} "
              f"folder={fold} mtime={filetime_to_str(cmtime)} res={res} size={size}")
        cdir = folder / fold
        manifest = cdir / f"container.{number}"
        if not manifest.exists():
            print(f"    !! missing {manifest.name}")
            continue
        m = Reader(manifest.read_bytes())
        mver, mcount = m.u32(), m.u32()
        actual = 0
        for _ in range(mcount):
            raw = m.data[m.pos:m.pos + 128]
            m.pos += 128
            bname = raw.decode("utf-16-le").split("\0", 1)[0]
            g1, g2 = m.guid(), m.guid()
            bfile = cdir / g1.hex.upper()
            if bfile.exists():
                data = bfile.read_bytes()
                actual += len(data)
                print(f"    blob {bname!r} file={g1.hex.upper()} size={len(data)} hash={blob_hash(data):08x} "
                      f"head={data[:8].hex()}{' alt!=file' if g1 != g2 else ''}")
            else:
                print(f"    blob {bname!r} file={g1.hex.upper()} !! MISSING")
        if actual != size:
            print(f"    !! index size {size} != files {actual}")
        extra = sorted(p.name for p in cdir.iterdir()) if cdir.exists() else []
        listed = {f"container.{number}"}
        # any stray files: old manifests or orphaned blobs
        stray = [n for n in extra if n not in listed and not any(n == b for b in [])]
        stray = [n for n in stray if n.startswith("container.")]
        if stray:
            print(f"    !! stray manifests: {stray}")
    if r.pos != len(r.data):
        print(f"!! {len(r.data) - r.pos} trailing bytes in index")
    print(f"total {total} bytes in {count} containers")


if __name__ == "__main__":
    main(sys.argv[1])
