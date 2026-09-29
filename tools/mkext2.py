#!/usr/bin/env python3
"""Deterministic EXT2 fixture builder for liftoff acceptance tests.

Layout (1 KiB blocks, rev 1, inode size 128, single block group):
  block 0        boot pad (1024 B)
  block 1        superblock (at byte offset 1024)
  block 2        group descriptor table
  block 3        block bitmap (unused by liftoff, left zero)
  block 4        inode bitmap (unused by liftoff, left zero)
  block 5..6     inode table (16 inodes)
  block 7..      directory blocks, then indirect tables, then file data

M10: arbitrary single-level tree via --extra ISO/PATH=HOST_FILE.
M13: files up to ~64 MiB (direct 12 + single indirect 256 + double indirect 256^2)
so a real 8.9 MiB kernel can live on the fixture, plus min_blocks for sizing the
filesystem inside a partition.
All bytes deterministic; identical inputs give identical images.
"""
import struct, argparse

BS = 1024
EXT2_MAGIC = 0xEF53
INODE_SIZE = 128
INODES = 16
INODE_TABLE_BLOCKS = (INODES * INODE_SIZE + BS - 1) // BS
BLOCK_BITMAP = 3
INODE_BITMAP = 4
INODE_TABLE = 5
DATA_START = INODE_TABLE + INODE_TABLE_BLOCKS
DIRECT = 12
MIN_DISK_BLOCKS = 2048  # 2 MiB floor, keeps the M2c fixture size stable


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
    assert next_ino <= INODES + 1, "too many inodes for the fixture"

    # Block assignment: directory blocks first, then indirect tables, then data.
    used_blocks = set(range(DATA_START))
    used_inodes = set(range(1, next_ino))   # inode 1 保留（坏块），2..next_ino-1 已用   # 0 引导垫 / 1 SB / 2 GDT / 3 块位图 / 4 inode 位图 / 5.. inode 表
    dir_blocks = {}
    cur = DATA_START
    order = [""] + sorted(dirs)
    for d in order:
        dir_blocks[d] = cur
        used_blocks.add(cur)
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
            used_blocks.update(data_blocks[key])
            if key in ind_block:
                used_blocks.add(ind_block[key])
            if key in dind_block:
                used_blocks.add(dind_block[key])
                used_blocks.update(dind_l1[key])
    total_blocks = max(cur, min_blocks)
    img = bytearray(total_blocks * BS)

    def w32(off, v):
        img[off:off + 4] = struct.pack("<I", v)

    def w16(off, v):
        img[off:off + 2] = struct.pack("<H", v)

    # Superblock @1024
    sb = BS
    w32(sb + 0, INODES)                 # s_inodes_count
    w32(sb + 4, total_blocks)           # s_blocks_count
    w32(sb + 20, 1)                     # s_first_data_block
    w32(sb + 24, 0)                     # s_log_block_size (1024)
    w32(sb + 28, 0)                     # s_log_frag_size
    w32(sb + 32, 8192)                  # s_blocks_per_group
    w32(sb + 40, 1024)                  # s_inodes_per_group
    w16(sb + 56, EXT2_MAGIC)
    w32(sb + 76, 1)                     # s_rev_level
    w16(sb + 88, INODE_SIZE)
    w32(sb + 12, total_blocks - len(used_blocks))   # s_free_blocks_count
    w32(sb + 16, INODES - len(used_inodes))         # s_free_inodes_count

    # Group descriptor table @block 2
    gd = 2 * BS
    w32(gd + 0, BLOCK_BITMAP)
    w32(gd + 4, INODE_BITMAP)
    w32(gd + 8, INODE_TABLE)

    # 位图与空闲计数：内核安装模式把启动分区**读写**挂为根，build_skeleton 会在
    # 其上创建骨架目录（分配 inode/块）。位图全零会让分配器看到「全部空闲」——
    # 与真实 mkfs 镜像不一致，实测内核在挂根前即停住。此处按实际占用写位图。
    bb = bytearray(BS)
    for b in used_blocks:
        bb[b // 8] |= 1 << (b % 8)
    img[BLOCK_BITMAP * BS:(BLOCK_BITMAP + 1) * BS] = bb
    ib = bytearray(BS)
    for i in used_inodes:
        ib[(i - 1) // 8] |= 1 << ((i - 1) % 8)
    img[INODE_BITMAP * BS:(INODE_BITMAP + 1) * BS] = ib
    w16(gd + 12, total_blocks - len(used_blocks))   # bg_free_blocks_count
    w16(gd + 14, INODES - len(used_inodes))         # bg_free_inodes_count

    itab = INODE_TABLE * BS

    def inode(ino: int) -> int:
        return itab + (ino - 1) * INODE_SIZE

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

    # Root inode + root directory
    write_dir_inode(2, BS, dir_blocks[""])
    root_entries = [(".", 2, 2), ("..", 2, 2)]
    for d in sorted(dirs):
        root_entries.append((d, ino_of[d], 2))
    img[dir_blocks[""] * BS:(dir_blocks[""] + 1) * BS] = pack_dir(root_entries)

    # Subdirectories + files
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