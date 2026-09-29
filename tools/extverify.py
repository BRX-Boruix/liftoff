#!/usr/bin/env python3
"""Verify an EXT2 fixture image by reading files back through the filesystem.

Independent of liftoff: parses the superblock/GDT/inodes, maps direct, single
indirect and double indirect blocks, then compares each requested file against
its host source byte for byte. Catches fixture-builder bugs (indirect-table
overflow, wrong sizes, bad offsets) before anything is booted.
"""
import struct, argparse

BS = 1024


def read_fs(path, base):
    d = open(path, "rb").read()

    def u32(o):
        return struct.unpack_from("<I", d, base + o)[0]

    def u16(o):
        return struct.unpack_from("<H", d, base + o)[0]

    sb = 1024
    blocks_count = u32(sb + 4)
    inode_size = u16(sb + 88)
    itab_blk = u32(2 * BS + 8)          # GDT[0].bg_inode_table
    itab = itab_blk * BS

    def stat(n):
        off = itab + (n - 1) * inode_size
        mode = u16(off)
        size = u32(off + 4)
        blocks = [u32(off + 40 + i * 4) for i in range(15)]
        return mode, size, blocks

    def blk(b):
        return d[base + b * BS: base + (b + 1) * BS]

    def map_logical(blocks, logical):
        if logical < 12:
            return blocks[logical]
        rem = logical - 12
        if rem < 256:
            l1 = blocks[12]
            if l1 == 0:
                return 0
            return struct.unpack_from("<I", blk(l1), rem * 4)[0]
        rem2 = rem - 256
        if rem2 >= 256 * 256:
            raise SystemExit("triple indirect not supported by this verifier")
        l2 = blocks[13]
        if l2 == 0:
            return 0
        l1 = struct.unpack_from("<I", blk(l2), (rem2 // 256) * 4)[0]
        if l1 == 0:
            return 0
        return struct.unpack_from("<I", blk(l1), (rem2 % 256) * 4)[0]

    def read_file(blocks, size):
        out = bytearray()
        for logical in range((size + BS - 1) // BS):
            b = map_logical(blocks, logical)
            out += blk(b) if b else bytes(BS)
        return bytes(out[:size])

    def lookup(dir_ino, name):
        _mode, size, blocks = stat(dir_ino)
        data = read_file(blocks, size)
        o = 0
        while o + 8 <= len(data):
            ino, rec, nlen, _ft = struct.unpack_from("<IHBB", data, o)
            if rec == 0:
                break
            if data[o + 8:o + 8 + nlen].decode("latin1") == name:
                return ino
            o += rec
        return None

    return {"stat": stat, "read_file": read_file, "lookup": lookup, "blocks_count": blocks_count}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", required=True)
    ap.add_argument("--base-sectors", type=int, default=0,
                    help="partition start in 512B sectors (0 = whole disk)")
    ap.add_argument("--file", action="append", default=[],
                    help="ISO/PATH=HOST_FILE to compare (repeatable)")
    a = ap.parse_args()
    fs = read_fs(a.image, a.base_sectors * 512)
    print(f"blocks_count={fs['blocks_count']}")
    bad = 0
    for spec in a.file:
        iso, host = spec.split("=", 1)
        ino = 2
        missing = None
        for comp in iso.lstrip("/").split("/"):
            ino = fs["lookup"](ino, comp)
            if ino is None:
                missing = comp
                break
        if missing is not None:
            print(f"FAIL {iso}: component {missing!r} not found")
            bad += 1
            continue
        _mode, size, blocks = fs["stat"](ino)
        data = fs["read_file"](blocks, size)
        src = open(host, "rb").read()
        if data == src:
            print(f"OK   {iso} size={size} sum16=0x{sum(data) & 0xFFFF:04x}")
        else:
            bad += 1
            print(f"FAIL {iso} size={size} host={len(src)}")
            for i in range(min(len(data), len(src))):
                if data[i] != src[i]:
                    print(f"     first diff at byte {i} (logical block {i // BS}):"
                          f" image={data[i]:#04x} host={src[i]:#04x}")
                    break
    raise SystemExit(1 if bad else 0)


if __name__ == "__main__":
    main()
