#!/usr/bin/env python3
"""Deterministic ISO9660 fixture builder for liftoff acceptance tests.

Builds a minimal ECMA-119 image with no Rock Ridge / Joliet extensions:
  LBA 0-15   system area (zeros)
  LBA 16     primary volume descriptor
  LBA 17     volume descriptor terminator
  LBA 18     root directory (".", "..", [KERNEL dir], [extra dirs])
  LBA 19     KERNEL directory (".", "..", KERNIMG.BIN)      [unless --flat]
  LBA 20..   one sector per extra directory (M8 modules), then file data

Directory entries never span sector boundaries (ECMA-119 6.8.1.1).
All bytes are deterministic; identical inputs give identical images.

M8: --extra ISO/PATH=HOST_FILE packs additional files (module fixtures).
"""
import struct, sys, os, argparse

SECTOR = 2048

def be32(v): return struct.pack(">I", v)
def lebe32(v): return struct.pack("<I", v) + struct.pack(">I", v)
def lebe16(v): return struct.pack("<H", v) + struct.pack(">H", v)

def rec(name: str, lba: int, size: int, is_dir: bool) -> bytes:
    """One directory record (ECMA-119 9.1)."""
    ident = name.encode("ascii")
    flags = 0x02 if is_dir else 0x00
    # length: 33 fixed + name; pad record to even total length (9.1.12)
    ln = 33 + len(ident)
    if ln % 2:
        ln += 1
    ident_padded = ident + (b"\x00" if (33 + len(ident)) % 2 else b"")
    dt = bytes([120, 1, 1, 1, 0, 0, 0])  # 2020-01-01 00:00:00 GMT
    r = (
        bytes([ln, 0]) + lebe32(lba) + lebe32(size)
        + dt + bytes([flags]) + bytes([0, 0])
        + lebe16(1) + bytes([len(ident)]) + ident_padded
    )
    assert len(r) == ln, (len(r), ln)
    return r

def pvd(root_lba: int, root_size: int, vol_ident: str, space_size: int) -> bytes:
    d = bytearray(2048)
    d[0] = 1
    d[1:6] = b"CD001"
    d[6] = 1
    d[8:40] = b"LIFTOFF".ljust(32).replace(b"\x00", b" ")
    d[40:72] = vol_ident.encode("ascii").ljust(32).replace(b"\x00", b" ")
    d[80:88] = lebe32(space_size)
    d[120:124] = lebe16(1)   # set size (both-endian)
    d[124:128] = lebe16(1)   # volume seq
    d[128:132] = lebe16(SECTOR)  # logical block size (both-endian, 8.4.10)
    root_rec = rec(".", root_lba, root_size, True)
    assert len(root_rec) == 34
    d[156:156+34] = root_rec
    return bytes(d)

def build(kernel: bytes, vol_ident: str, flat: bool = False, extras=None) -> bytes:
    """extras: list of (iso_dir, iso_name, data); iso_dir == "" means root."""
    extras = extras or []
    root_dir = 18
    root_size = 2048
    kern_dir_lba = 19

    # 分组：目录名 -> [(name, data)]，保持插入序；根目录文件单独处理。
    dir_order = []
    dir_files = {}
    root_files = []
    for d, n, data in extras:
        if d == "":
            root_files.append((n, data))
        else:
            if d not in dir_files:
                dir_files[d] = []
                dir_order.append(d)
            dir_files[d].append((n, data))

    dir_lbas = {}
    next_lba = 20
    for d in dir_order:
        dir_lbas[d] = next_lba
        next_lba += 1
    data_lba = next_lba

    # 数据区分配
    def sectors(n): return (n + SECTOR - 1) // SECTOR
    kern_lba = data_lba
    cur = data_lba + sectors(len(kernel))
    file_lba = {}
    for d, n, data in extras:
        key = (d, n)
        if key in file_lba:
            raise SystemExit('duplicate ISO path in --extra: ' + str(key))
        file_lba[key] = cur
        cur += sectors(len(data))
    space = cur

    # 根目录
    root = bytearray()
    root += rec(".", root_dir, root_size, True)
    root += rec("..", root_dir, root_size, True)
    if not flat:
        root += rec("KERNEL", kern_dir_lba, root_size, True)
    for d in dir_order:
        root += rec(d, dir_lbas[d], root_size, True)
    for n, data in root_files:
        root += rec(n, file_lba[("", n)], len(data), False)
    assert len(root) <= root_size, len(root)
    root += bytes(root_size - len(root))

    # KERNEL 目录
    kdir = bytearray()
    kdir += rec(".", kern_dir_lba, root_size, True)
    kdir += rec("..", root_dir, root_size, True)
    kdir += rec("KERNIMG.BIN", kern_lba, len(kernel), False)
    assert len(kdir) <= root_size, len(kdir)
    kdir += bytes(root_size - len(kdir))

    # 附加目录
    dir_sectors = {}
    for d in dir_order:
        sec = bytearray()
        sec += rec(".", dir_lbas[d], root_size, True)
        sec += rec("..", root_dir, root_size, True)
        for n, data in dir_files[d]:
            sec += rec(n, file_lba[(d, n)], len(data), False)
        assert len(sec) <= root_size, (d, len(sec))
        sec += bytes(root_size - len(sec))
        dir_sectors[d] = bytes(sec)

    img = bytearray(space * SECTOR)
    img[16 * SECTOR:17 * SECTOR] = pvd(root_dir, root_size, vol_ident, space)
    term = bytearray(2048); term[0] = 255; term[1:6] = b"CD001"; term[6] = 1
    img[17 * SECTOR:18 * SECTOR] = term
    img[root_dir * SECTOR:(root_dir + 1) * SECTOR] = root
    img[kern_dir_lba * SECTOR:(kern_dir_lba + 1) * SECTOR] = kdir
    for d in dir_order:
        img[dir_lbas[d] * SECTOR:(dir_lbas[d] + 1) * SECTOR] = dir_sectors[d]
    img[kern_lba * SECTOR:kern_lba * SECTOR + len(kernel)] = kernel
    for d, n, data in extras:
        lba = file_lba[(d, n)]
        img[lba * SECTOR:lba * SECTOR + len(data)] = data
    return bytes(img), len(kernel)

def parse_extra(spec: str):
    """ISO/PATH=HOST_FILE -> (dir, name, data)"""
    if "=" not in spec:
        raise SystemExit("bad --extra (want ISO/PATH=HOST_FILE): " + spec)
    iso, host = spec.split("=", 1)
    iso = iso.lstrip("/")
    parts = iso.split("/")
    if len(parts) == 1:
        d, n = "", parts[0]
    elif len(parts) == 2:
        d, n = parts[0], parts[1]
    else:
        raise SystemExit("bad --extra path (one dir level only): " + iso)
    with open(host, "rb") as f:
        return (d, n, f.read())

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True, help="payload file")
    ap.add_argument("--out", required=True, help="output .iso path")
    ap.add_argument("--volident", default="LIFTOFF_M2B")
    ap.add_argument("--flat", action="store_true", help="root without KERNEL dir (negative test)")
    ap.add_argument("--extra", action="append", default=[], help="ISO/PATH=HOST_FILE (repeatable)")
    a = ap.parse_args()
    with open(a.kernel, "rb") as f:
        payload = f.read()
    extras = [parse_extra(s) for s in a.extra]
    img, fsize = build(payload, a.volident, flat=a.flat, extras=extras)
    with open(a.out, "wb") as f:
        f.write(img)
    ln = len(img)
    s = sum(payload) & 0xFFFF
    print(f"iso_bytes={ln} file_len={fsize} file_sum16=0x{s:04x}")
    for d, n, data in extras:
        print(f"extra={d}/{n} len={len(data)} sum16=0x{sum(data) & 0xFFFF:04x}")

if __name__ == "__main__":
    main()