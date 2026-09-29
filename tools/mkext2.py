#!/usr/bin/env python3
"""Deterministic EXT2 fixture builder for liftoff M2c acceptance tests.

Layout mirrors kernel/crates/fs/src/ext2.rs build_image():
  2 MiB raw disk (whole-disk FS, no MBR — the M2c drive is exposed as one
  BlockIo device; MBR partition handling is deferred to M4 boot-source logic)
  block size 1024, rev 1, inode_size 128
  block 0  : boot pad + superblock @1024
  block 2  : group descriptor table (block_bitmap=3, inode_bitmap=4, inode_table=5)
  block 5  : inode table (root=ino2 @128, BOOT dir=ino11 @1280, file=ino12 @1408)
  block 20 : root dir data (".", "..", BOOT)
  block 21 : BOOT dir data (".", "..", KERNIMG.BIN)
  block 25 : file payload (36 bytes)
All bytes deterministic.
"""
import struct, sys, argparse

BS = 1024
DISK_SECTORS = 4096  # 2 MiB

EXT2_MAGIC = 0xEF53

def de(name: str, ino: int, ft: int, rec: int) -> bytes:
    """One directory entry: ino(4) rec_len(2) name_len(1) file_type(1) name."""
    nb = name.encode("ascii")
    r = struct.pack("<IHBB", ino, rec, len(nb), ft) + nb
    assert len(r) <= rec
    return r + bytes(rec - len(r))

def build(payload: bytes, flat: bool = False) -> bytes:
    assert len(payload) <= BS
    img = bytearray(DISK_SECTORS * 512)

    def w32(off, v): img[off:off+4] = struct.pack("<I", v)
    def w16(off, v): img[off:off+2] = struct.pack("<H", v)

    # Superblock @1024
    sb = 1024
    w32(sb + 0, 1024)        # s_inodes_count
    w32(sb + 4, DISK_SECTORS * 512 // BS)  # s_blocks_count = 2048
    w32(sb + 20, 1)          # s_first_data_block
    w32(sb + 24, 0)          # s_log_block_size (1024)
    w32(sb + 28, 0)          # s_log_frag_size
    w32(sb + 32, 8192)       # s_blocks_per_group
    w32(sb + 40, 1024)       # s_inodes_per_group
    w16(sb + 56, EXT2_MAGIC) # s_magic
    w32(sb + 76, 1)          # s_rev_level
    w16(sb + 88, 128)        # s_inode_size

    # GDT @block2 (offset 2*1024): inode_table=5, bitmaps 3/4
    gd = 2 * BS
    w32(gd + 0, 3)   # bg_block_bitmap
    w32(gd + 4, 4)   # bg_inode_bitmap
    w32(gd + 8, 5)   # bg_inode_table

    # Inode table @block5. inode N at 5*BS + (N-1)*128.
    itab = 5 * BS
    root = itab + 1 * 128
    w16(root + 0, 0x4000 | 0o755)  # dir
    w32(root + 4, BS)              # size
    w32(root + 40 + 0, 20)         # block[0]
    boot = itab + 10 * 128
    w16(boot + 0, 0x4000 | 0o755)  # dir
    w32(boot + 4, BS)
    w32(boot + 40 + 0, 21)
    kern = itab + 11 * 128
    w16(kern + 0, 0x8000 | 0o644)  # regular file
    w32(kern + 4, len(payload))
    w32(kern + 40 + 0, 25)

    # Directory data blocks
    if flat:
        # 最后一项 rec_len 必须覆盖到块尾（EXT2 目录块无尾部零填充语义）
        root_entries = de(".", 2, 2, 12) + de("..", 2, 2, BS - 12)
    else:
        root_entries = de(".", 2, 2, 12) + de("..", 2, 2, 12) + de("BOOT", 11, 2, BS - 24)
    rootd = root_entries
    img[20*BS:21*BS] = rootd + bytes(BS - len(rootd))
    bootd = de(".", 11, 2, 12) + de("..", 2, 2, 12) + de("KERNIMG.BIN", 12, 1, BS - 24)
    img[21*BS:22*BS] = bootd + bytes(BS - len(bootd))
    img[25*BS:25*BS+len(payload)] = payload
    return bytes(img)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--flat", action="store_true", help="root without BOOT dir (negative test)")
    a = ap.parse_args()
    payload = open(a.kernel, "rb").read()
    img = build(payload, flat=a.flat)
    open(a.out, "wb").write(img)
    s = sum(payload) & 0xFFFF
    print(f"ext_bytes={len(img)} file_len={len(payload)} file_sum16=0x{s:04x}")

if __name__ == "__main__":
    main()