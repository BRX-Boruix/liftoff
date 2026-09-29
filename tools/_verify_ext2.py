import struct, sys
BS = 1024
img = open(sys.argv[1], "rb").read()
expect_payload = open(sys.argv[2], "rb").read()

sb = 1024
assert struct.unpack("<H", img[sb+56:sb+58])[0] == 0xEF53, "magic"
log_bs = struct.unpack("<I", img[sb+24:sb+28])[0]
assert log_bs == 0 and BS == 1024
fdb = struct.unpack("<I", img[sb+20:sb+24])[0]
assert fdb == 1
ipg = struct.unpack("<I", img[sb+40:sb+44])[0]
isize = struct.unpack("<H", img[sb+88:sb+90])[0]
assert isize == 128

gdt = (fdb + 1) * BS
ino_table = struct.unpack("<I", img[gdt+8:gdt+12])[0]

def read_ino(n):
    off = ino_table * BS + (n - 1) * isize
    mode = struct.unpack("<H", img[off:off+2])[0]
    size = struct.unpack("<I", img[off+4:off+8])[0]
    blocks = struct.unpack("<12I", img[off+40:off+88])
    return mode, size, blocks

mode, size, blocks = read_ino(2)
assert mode & 0xF000 == 0x4000, "root is dir"
root = img[blocks[0]*BS:blocks[0]*BS+size]

off = 0; found = None
while off < len(root):
    ino = struct.unpack("<I", root[off:off+4])[0]
    rec = struct.unpack("<H", root[off+4:off+6])[0]
    nl = root[off+6]
    if ino != 0 and root[off+8:off+8+nl] == b"BOOT":
        found = ino; break
    off += rec
assert found == 11, "BOOT dir entry"

mode, size, blocks = read_ino(11)
assert mode & 0xF000 == 0x4000
bootd = img[blocks[0]*BS:blocks[0]*BS+size]
off = 0; found = None
while off < len(bootd):
    ino = struct.unpack("<I", bootd[off:off+4])[0]
    rec = struct.unpack("<H", bootd[off+4:off+6])[0]
    nl = bootd[off+6]
    if ino != 0 and bootd[off+8:off+8+nl] == b"KERNIMG.BIN":
        found = ino; break
    off += rec
assert found == 12, "KERNIMG.BIN entry"

mode, size, blocks = read_ino(12)
assert mode & 0xF000 == 0x8000, "regular file"
data = img[blocks[0]*BS:blocks[0]*BS+size]
assert data == expect_payload, "payload mismatch"
print("VALID len=%d sum16=0x%04x" % (len(data), sum(data) & 0xFFFF))
