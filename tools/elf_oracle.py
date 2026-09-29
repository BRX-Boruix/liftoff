#!/usr/bin/env python3
"""ELF load oracle: computes the exact values liftoff's M3 chain must report.

Same parse rules as src/elf.rs (single source of expectations):
  - ELF64 little-endian, ET_EXEC or ET_DYN, EM_X86_64
  - load model: one contiguous physical image; segments placed at
    image_base + (p_vaddr - min_vaddr), filesz bytes copied, rest zeroed
  - per-segment sum16 covers filesz file bytes; zero tail contributes 0
"""
import struct, sys, argparse

PT_LOAD = 1

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("elf")
    a = ap.parse_args()
    d = open(a.elf, "rb").read()
    assert d[0:4] == b"\x7fELF", "magic"
    assert d[4] == 2, "class 64"
    assert d[5] == 1, "LE"
    etype, machine = struct.unpack_from("<HH", d, 16)
    assert machine == 62, "EM_X86_64"
    entry, phoff = struct.unpack_from("<QQ", d, 24)
    phentsize, phnum = struct.unpack_from("<HH", d, 54)
    assert phentsize == 56
    loads = []
    for i in range(phnum):
        off = phoff + i * phentsize
        p_type, p_flags, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align = struct.unpack_from("<IIQQQQQQ", d, off)
        if p_type != PT_LOAD or p_memsz == 0:
            continue
        assert p_filesz <= p_memsz, "filesz > memsz"
        assert p_offset + p_filesz <= len(d), "segment beyond file"
        loads.append((p_vaddr, p_offset, p_filesz, p_memsz))
    assert loads, "no PT_LOAD"
    min_vaddr = min(v for v, _, _, _ in loads)
    max_end = max(v + m for v, _, _, m in loads)
    segs = 0; total_mem = 0; total_sum = 0
    for v, o, fsz, msz in loads:
        segs += 1
        s = sum(d[o:o + fsz]) & 0xFFFF
        total_mem += msz
        total_sum = (total_sum + s) & 0xFFFF
        print("M3: seg{} rel=0x{:x} size={} sum16=0x{:04x}".format(segs - 1, v - min_vaddr, msz, s))
    print("M3: segs={} entry=0x{:016x}".format(segs, entry))
    print("M3: vbase=0x{:016x} image_size={}".format(min_vaddr, max_end - min_vaddr))
    print("M3: total_mem={} total_sum16=0x{:04x}".format(total_mem, total_sum))

if __name__ == "__main__":
    main()
