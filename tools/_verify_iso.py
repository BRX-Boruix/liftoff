import struct, sys
SECTOR = 2048
data = open(sys.argv[1], "rb").read()
assert data[16*SECTOR:16*SECTOR+6] == b"\x01CD001", "PVD magic"
pvd = data[16*SECTOR:17*SECTOR]
root_lba, root_size = struct.unpack('<I', pvd[158:162])[0], struct.unpack('<I', pvd[166:170])[0]
print("root", root_lba, root_size)
def entries(buf):
    out, i = [], 0
    while i < len(buf) and buf[i] != 0:
        ln = buf[i]
        e = buf[i:i+ln]
        lba, size = struct.unpack("<I", e[2:6])[0], struct.unpack("<I", e[10:14])[0]
        flags, nid = e[25], e[32]
        out.append((e[33:33+nid].decode("ascii"), lba, size, flags))
        i += ln
    return out
root = data[root_lba*SECTOR:(root_lba+1)*SECTOR]
for e in entries(root): print("  root:", e)
kd = [e for e in entries(root) if e[0] == "KERNEL"][0]
kbuf = data[kd[1]*SECTOR:(kd[1]+1)*SECTOR]
for e in entries(kbuf): print("  kernel-dir:", e)
f = [e for e in entries(kbuf) if e[0] == "KERNIMG.BIN"][0]
payload = data[f[1]*SECTOR:f[1]*SECTOR+f[2]]
s = sum(payload) & 0xFFFF
print("file", f[1], f[2], hex(s))
assert payload == open(sys.argv[2], "rb").read(), "payload mismatch"
print("VALID")