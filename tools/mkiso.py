#!/usr/bin/env python3
"""Deterministic ISO9660 fixture builder for liftoff M2b acceptance tests.

Builds a minimal ECMA-119 image with no Rock Ridge / Joliet extensions:
  LBA 0-15   system area (zeros)
  LBA 16     primary volume descriptor
  LBA 17     volume descriptor terminator
  LBA 18     root directory (2 + 1 entries: ".", "..", KERNEL)
  LBA 19     KERNEL directory (2 + 1 entries: ".", "..", KERNIMG.BIN)
  LBA 20..   file data

Directory entries never span sector boundaries (ECMA-119 6.8.1.1).
All bytes are deterministic; identical inputs give identical images.
"""
import struct, sys, argparse

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
    # 8.4.18: root_directory_record embedded at 156, same layout as a dir record,
    # 34 bytes for the '.' entry. Both-endian fields come from rec().
    root_rec = rec(".", root_lba, root_size, True)
    assert len(root_rec) == 34
    d[156:156+34] = root_rec
    return bytes(d)

def build(kernel: bytes, vol_ident: str, flat: bool = False) -> bytes:
    root_entries_len = (len(rec(".", 0, 0, True)) * 2
                        + len(rec("KERNEL", 0, 0, True)))
    root_size = 2048  # one sector, padded
    kern_dir_lba = 19
    file_lba = 20
    file_size = len(kernel)
    file_sectors = (file_size + SECTOR - 1) // SECTOR

    root = bytearray()
    root += rec(".", 18, root_size, True)
    root += rec("..", 18, root_size, True)
    if not flat:
        root += rec("KERNEL", kern_dir_lba, root_size, True)
    assert len(root) <= root_size
    root += bytes(root_size - len(root))  # pad: entries must not span sectors (ECMA-119 6.8.1.1)

    kdir = bytearray()
    kdir += rec(".", kern_dir_lba, root_size, True)
    kdir += rec("..", 18, root_size, True)
    kdir += rec("KERNIMG.BIN", file_lba, file_size, False)
    assert len(kdir) <= root_size
    kdir += bytes(root_size - len(kdir))  # pad: same rule

    space = 20 + file_sectors
    img = bytearray(space * SECTOR)
    img[16 * SECTOR:17 * SECTOR] = pvd(18, root_size, vol_ident, space)
    term = bytearray(2048); term[0] = 255; term[1:6] = b"CD001"; term[6] = 1
    img[17 * SECTOR:18 * SECTOR] = term
    img[18 * SECTOR:19 * SECTOR] = root
    img[kern_dir_lba * SECTOR:(kern_dir_lba + 1) * SECTOR] = kdir
    img[file_lba * SECTOR:file_lba * SECTOR + file_size] = kernel
    return bytes(img), file_size

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True, help="payload file")
    ap.add_argument("--out", required=True, help="output .iso path")
    ap.add_argument("--volident", default="LIFTOFF_M2B")
    ap.add_argument("--flat", action="store_true", help="root without KERNEL dir (negative test)")
    a = ap.parse_args()
    with open(a.kernel, "rb") as f:
        payload = f.read()
    img, fsize = build(payload, a.volident, flat=a.flat)
    with open(a.out, "wb") as f:
        f.write(img)
    ln = len(img)
    s = sum(payload) & 0xFFFF
    print(f"iso_bytes={ln} file_len={fsize} file_sum16=0x{s:04x}")

if __name__ == "__main__":
    main()