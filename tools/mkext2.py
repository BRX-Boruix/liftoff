#!/usr/bin/env python3
"""Deterministic EXT2 fixture builder for liftoff acceptance tests.

Layout (1 KiB blocks, rev 1, inode size 128, **multi-group**):
  block 0            boot pad (1024 B)
  block 1            superblock (at byte offset 1024)
  block 2            group descriptor table (one 32 B entry per group)
  block 3..          per group: block bitmap, inode bitmap, inode table (2 blocks)
  then               directory blocks, indirect tables, file data

Multi-group metadata matters because the kernel mounts the boot partition
read-write in install mode: build_skeleton allocates inodes and blocks, so the
bitmaps, per-group free counts and the group descriptors must describe reality
(zeroed bitmaps made the kernel stop before printing its install-mode root line).

M10: arbitrary single-level tree via --extra ISO/PATH=HOST_FILE.
M13: double indirect blocks (files up to ~64 MiB) so a real 8.9 MiB kernel fits,
and min_blocks to size the filesystem inside a partition.
All bytes deterministic for a given input.
"""
import struct, argparse

BS = 1024
EXT2_MAGIC = 0xEF53
INODE_SIZE = 128

# EXT2 经典约束：块位图必须装进一个块 → 每组块数 ≤ 8 × 块大小（1KiB 块 → 8192）。
BLOCKS_PER_GROUP = 8192
INODES_PER_GROUP = 16
INODE_TABLE_BLOCKS = (INODES_PER_GROUP * INODE_SIZE + BS - 1) // BS
GROUP_META_BLOCKS = 2 + INODE_TABLE_BLOCKS          # 块位图 + inode 位图 + inode 表
GDT_BLOCK = 2
FIRST_META_BLOCK = 3
DIRECT = 12
MIN_DISK_BLOCKS = 2048


def de(name: str, ino: int, ft: int, rec: int) -> bytes:
    """One directory entry: ino(4) rec_len(2) name_len(1) file_type(1) name."""
    nb = name.encode("ascii")
    r = struct.pack("<IHBB", ino, rec, len(nb), ft) + nb
    assert len(r) <= rec, (name, rec)
    return r + bytes(rec - len(r))


def pack_dir(entries) -> bytes:
    """entries: [(name, ino, file_type)] -> one 1024 B directory block.

    The final entry absorbs the remaining space (EXT2 has no trailing zero
    padding inside a directory block)."""
    out = b""
    for i, (name, ino, ft) in enumerate(entries):
        need = (8 + len(name) + 3) & ~3
        rec = (BS - len(out)) if i == len(entries) - 1 else need
        out += de(name, ino, ft, rec)
    assert len(out) == BS, len(out)
    return out


def build(kernel: bytes, extras, flat: bool = False, min_blocks: int = MIN_DISK_BLOCKS) -> bytes:
    """kernel -> BOOT/KERNIMG.BIN (omitted when flat, e.g. when the caller wants the
    canonical install path boot/kernel via extras); extras: [(dir, name, data)]."""
    dirs = {}          # dir name -> [(name, data)]
    if not flat:
        dirs["BOOT"] = [("KERNIMG.BIN", kernel)]
    for d, n, data in extras:
        dirs.setdefault(d, []).append((n, data))

    groups = max(1, (min_blocks + BLOCKS_PER_GROUP - 1) // BLOCKS_PER_GROUP)
    data_start = FIRST_META_BLOCK + groups * GROUP_META_BLOCKS
    inodes_count = groups * INODES_PER_GROUP

    # Inode assignment: 2 = root, then dirs and files in sorted order.
    ino_of = {"": 2}
    next_ino = 11
    for d in sorted(dirs):
        ino_of[d] = next_ino
        next_ino += 1
    file_ino = {}
    for d in sorted(dirs):
        for n, _ in sorted(dirs[d]):
            file_ino[(d, n)] = next_ino
            next_ino += 1
    assert next_ino <= inodes_count + 1, "too many inodes for the fixture"

    # Block assignment: directory blocks, then indirect tables, then data.
    cur = data_start
    dir_blocks = {}
    for d in [""] + sorted(dirs):
        dir_blocks[d] = cur
        cur += 1
    ind_block = {}
    dind_block = {}
    dind_l1 = {}
    data_blocks = {}
    for d in sorted(dirs):
        for n, data in sorted(dirs[d]):
            key = (d, n)
            nb = (len(data) + BS - 1) // BS
            if nb > DIRECT:
                ind_block[key] = cur
                cur += 1
            if nb > DIRECT + 256:
                # 二级间接：l2 表（每项一个 l1 表块）+ 每个 l1 表 256 个数据块号
                dind_block[key] = cur
                cur += 1
                nl1 = (nb - DIRECT - 256 + 255) // 256
                dind_l1[key] = list(range(cur, cur + nl1))
                cur += nl1
            data_blocks[key] = list(range(cur, cur + nb))
            cur += nb
    total_blocks = max(cur, min_blocks)
    if (total_blocks + BLOCKS_PER_GROUP - 1) // BLOCKS_PER_GROUP != groups:
        raise SystemExit("fixture outgrew its block-group count")

    # 占用集合：元数据块 + 目录块 + 间接表 + 数据块；inode 1 保留（坏块）。
    used_blocks = set(range(data_start))
    used_blocks.update(dir_blocks.values())
    for key, blocks in data_blocks.items():
        used_blocks.update(blocks)
        if key in ind_block:
            used_blocks.add(ind_block[key])
        if key in dind_block:
            used_blocks.add(dind_block[key])
            used_blocks.update(dind_l1[key])
    used_inodes = set(range(1, next_ino))

    img = bytearray(total_blocks * BS)

    def w32(off, v):
        img[off:off + 4] = struct.pack("<I", v)

    def w16(off, v):
        img[off:off + 2] = struct.pack("<H", v)

    # Superblock @1024
    sb = BS
    w32(sb + 0, inodes_count)                 # s_inodes_count
    w32(sb + 4, total_blocks)                 # s_blocks_count
    w32(sb + 12, total_blocks - len(used_blocks))   # s_free_blocks_count
    w32(sb + 16, inodes_count - len(used_inodes))   # s_free_inodes_count
    w32(sb + 20, 1)                           # s_first_data_block
    w32(sb + 24, 0)                           # s_log_block_size (1024)
    w32(sb + 28, 0)                           # s_log_frag_size
    w32(sb + 32, BLOCKS_PER_GROUP)            # s_blocks_per_group
    w32(sb + 40, INODES_PER_GROUP)            # s_inodes_per_group
    w16(sb + 56, EXT2_MAGIC)
    w32(sb + 76, 1)                           # s_rev_level
    w16(sb + 88, INODE_SIZE)

    # 组描述符表（每组 32 B）与各组位图 / 空闲计数。
    for g in range(groups):
        goff = GDT_BLOCK * BS + g * 32
        bb = FIRST_META_BLOCK + g * GROUP_META_BLOCKS
        w32(goff + 0, bb)                     # bg_block_bitmap
        w32(goff + 4, bb + 1)                 # bg_inode_bitmap
        w32(goff + 8, bb + 2)                 # bg_inode_table
        gstart = g * BLOCKS_PER_GROUP
        gend = min(gstart + BLOCKS_PER_GROUP, total_blocks)
        used_b = sum(1 for b in used_blocks if gstart <= b < gend)
        w16(goff + 12, (gend - gstart) - used_b)
        lo = g * INODES_PER_GROUP + 1
        hi = (g + 1) * INODES_PER_GROUP
        used_i = sum(1 for i in used_inodes if lo <= i <= hi)
        w16(goff + 14, INODES_PER_GROUP - used_i)
        bm = bytearray(BS)
        for b in used_blocks:
            if gstart <= b < gstart + BLOCKS_PER_GROUP:
                i = b - gstart
                bm[i // 8] |= 1 << (i % 8)
        img[bb * BS:(bb + 1) * BS] = bm
        im = bytearray(BS)
        for i in used_inodes:
            if lo <= i <= hi:
                j = i - lo
                im[j // 8] |= 1 << (j % 8)
        img[(bb + 1) * BS:(bb + 2) * BS] = im

    def inode(ino: int) -> int:
        g = (ino - 1) // INODES_PER_GROUP
        idx = (ino - 1) % INODES_PER_GROUP
        table = FIRST_META_BLOCK + g * GROUP_META_BLOCKS + 2
        return table * BS + idx * INODE_SIZE

    def write_dir_inode(ino: int, size: int, block: int):
        off = inode(ino)
        w16(off + 0, 0x4000 | 0o755)
        w32(off + 4, size)
        w32(off + 40, block)

    def write_file_inode(ino: int, data: bytes, key) -> None:
        off = inode(ino)
        w16(off + 0, 0x8000 | 0o644)
        w32(off + 4, len(data))
        blocks = data_blocks[key]
        for i, b in enumerate(blocks[:DIRECT]):
            w32(off + 40 + i * 4, b)
        if len(blocks) > DIRECT:
            w32(off + 40 + 12 * 4, ind_block[key])
            rest = blocks[DIRECT:DIRECT + 256]
            tbl = b"".join(struct.pack("<I", b) for b in rest)
            base = ind_block[key] * BS
            img[base:base + len(tbl)] = tbl
        if len(blocks) > DIRECT + 256:
            w32(off + 40 + 13 * 4, dind_block[key])
            l2 = b"".join(struct.pack("<I", b) for b in dind_l1[key])
            base = dind_block[key] * BS
            img[base:base + len(l2)] = l2
            rest = blocks[DIRECT + 256:]
            for i, l1b in enumerate(dind_l1[key]):
                chunk = rest[i * 256:(i + 1) * 256]
                t = b"".join(struct.pack("<I", b) for b in chunk)
                b0 = l1b * BS
                img[b0:b0 + len(t)] = t
        for i, b in enumerate(blocks):
            off_b = b * BS
            start = i * BS
            img[off_b:off_b + len(data[start:start + BS])] = data[start:start + BS]

    # 根目录与各子目录
    write_dir_inode(2, BS, dir_blocks[""])
    root_entries = [(".", 2, 2), ("..", 2, 2)]
    for d in sorted(dirs):
        root_entries.append((d, ino_of[d], 2))
    img[dir_blocks[""] * BS:(dir_blocks[""] + 1) * BS] = pack_dir(root_entries)
    for d in sorted(dirs):
        write_dir_inode(ino_of[d], BS, dir_blocks[d])
        entries = [(".", ino_of[d], 2), ("..", 2, 2)]
        for n, _ in sorted(dirs[d]):
            entries.append((n, file_ino[(d, n)], 1))
        img[dir_blocks[d] * BS:(dir_blocks[d] + 1) * BS] = pack_dir(entries)
        for n, data in sorted(dirs[d]):
            write_file_inode(file_ino[(d, n)], data, (d, n))

    return bytes(img)


def parse_extra(spec: str):
    if "=" not in spec:
        raise SystemExit("bad --extra (want ISO/PATH=HOST_FILE): " + spec)
    iso, host = spec.split("=", 1)
    parts = iso.lstrip("/").split("/")
    if len(parts) != 2:
        raise SystemExit("bad --extra path (one dir level only): " + iso)
    with open(host, "rb") as f:
        return (parts[0], parts[1], f.read())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--flat", action="store_true",
                    help="skip the default BOOT/KERNIMG.BIN (caller supplies files via --extra)")
    ap.add_argument("--extra", action="append", default=[], help="ISO/PATH=HOST_FILE (repeatable)")
    a = ap.parse_args()
    with open(a.kernel, "rb") as f:
        payload = f.read()
    extras = [parse_extra(s) for s in a.extra]
    img = build(payload, extras, flat=a.flat)
    with open(a.out, "wb") as f:
        f.write(img)
    s = sum(payload) & 0xFFFF
    print(f"ext_bytes={len(img)} file_len={len(payload)} file_sum16=0x{s:04x}")
    for d, n, data in extras:
        print(f"extra={d}/{n} len={len(data)} sum16=0x{sum(data) & 0xFFFF:04x}")


if __name__ == "__main__":
    main()
