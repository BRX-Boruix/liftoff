#!/usr/bin/env python3
"""Deterministic MBR + EXT2 system-disk builder for liftoff install-mode tests.

Mirrors the layout the project toolchain produces (tools/main.py build
--systemdisk) so liftoff and the kernel exercise exactly the same path:
  MBR   signature 0xAA55, disk signature 0x424F5255 ("BORU"),
        partition 1: status 0x80, type 0x83 (Linux), start LBA 2048
  EXT2  1 KiB blocks inside the partition, /boot/kernel = the kernel ELF
CHS fields are written as zeros: the kernel MBR parser ignores them by design
("LBA 是唯一寻址事实") and liftoff reads only status/type/LBA/sectors.
All bytes deterministic for a given kernel.
"""
import argparse, struct

import mkext2

SECTOR = 512
BS = 1024


def build(kernel: bytes, size_mib: int, part_start_lba: int, disk_id: int) -> bytes:
    disk_bytes = size_mib * 1024 * 1024
    part_off = part_start_lba * SECTOR
    part_bytes = disk_bytes - part_off
    if part_bytes <= 0:
        raise SystemExit("partition start is beyond the disk")
    # 文件系统铺满分区（块数与分区容量一致，与规范系统盘同口径），内核放在
    # 规范安装路径 /boot/kernel（与 tools/limine.conf 的 kernel_path 一致）。
    fs = mkext2.build(b"", [("boot", "kernel", kernel)], flat=True, min_blocks=part_bytes // BS)
    if len(fs) > part_bytes:
        raise SystemExit(f"kernel does not fit: fs={len(fs)} part={part_bytes}")
    img = bytearray(disk_bytes)
    img[part_off:part_off + len(fs)] = fs
    e = 0x1BE                     # partition entry 1
    img[e + 0] = 0x80             # bootable
    img[e + 4] = 0x83             # Linux (EXT2)
    struct.pack_into("<I", img, e + 8, part_start_lba)
    struct.pack_into("<I", img, e + 12, len(fs) // SECTOR)
    struct.pack_into("<I", img, 0x1B8, disk_id)
    img[510] = 0x55
    img[511] = 0xAA
    return bytes(img)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kernel", required=True, help="kernel ELF to install as /boot/kernel")
    ap.add_argument("--out", required=True)
    ap.add_argument("--size-mib", type=int, default=64)
    ap.add_argument("--part-start-lba", type=int, default=2048)
    ap.add_argument("--disk-id", default="0x424F5255")
    a = ap.parse_args()
    kernel = open(a.kernel, "rb").read()
    disk_id = int(a.disk_id, 0)
    img = build(kernel, a.size_mib, a.part_start_lba, disk_id)
    open(a.out, "wb").write(img)
    print(f"sysdisk bytes={len(img)} disk_id={disk_id:#010x}"
          f" part_start_lba={a.part_start_lba} part_sectors={(len(img) - a.part_start_lba * SECTOR) // SECTOR}"
          f" kernel_len={len(kernel)} sum16=0x{sum(kernel) & 0xFFFF:04x}")


if __name__ == "__main__":
    main()