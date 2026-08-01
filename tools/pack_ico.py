#!/usr/bin/env python3
"""Pack the six PNG icon sizes into icon.ico (PNG-compressed entries, Win Vista+).
Usage: python tools/pack_ico.py <out.ico> <size.png> [<size.png> ...]"""
import struct, sys

def main(out, files):
    sizes = []
    blobs = []
    for f in files:
        data = open(f, "rb").read()
        # size is the leading number of the filename
        s = int(f.rsplit("\\", 1)[-1].rsplit("/", 1)[-1].split("_")[1].split(".")[0])
        sizes.append(s)
        blobs.append(data)
    count = len(sizes)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    for s, data in zip(sizes, blobs):
        dim = 0 if s == 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(out, "wb") as f:
        f.write(header)
        f.write(entries)
        for data in blobs:
            f.write(data)
    print(f"wrote {out}: {len(sizes)} entries {sorted(sizes)}")

if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2:])
