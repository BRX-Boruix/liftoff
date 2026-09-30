//! EXT2 只读解析器（布局事实源：内核 crates/fs/src/ext2.rs 取证版）。
//!
//! 只读子集的边界（有意为之，不是偷工）：
//! - 块大小固定 1024（log_block_size==0）：fixture 与安装镜像统一口径，
//!   其他块大小显式拒绝，不做静默适配
//! - 数据寻址：直接块 12 + 一级间接；二级/三重间接出现即报错
//!   （内核路径文件 ≤12KiB 时用不到间接；M3 内核超过 12KiB 再扩展）
//! - 目录缓冲上限一个块（1024B）：根目录/BOOT 目录单块即可容纳；
//!   超过即报错，不做动态扩容（fixture 与安装镜像口径）
//! - 无 MBR：M2c 把整盘设备当文件系统挂；分区解析属 M4 BootSource 逻辑
//!
//! 复用 [`crate::iso9660::BlockRead`] 与 `PoolBuf`；不触碰 UEFI。

use crate::efi;
use crate::iso9660::BlockRead;

/// 超级块魔数（@56，u16）。
const EXT2_MAGIC: u16 = 0xEF53;

/// 根目录 inode 号（规范固定）。
const ROOT_INO: u32 = 2;

/// inode 内 i_block 数组偏移（15 × u32 = 60 字节）。
const INODE_BLOCK_ARRAY: usize = 40;

/// mode 高 4 位：目录。
const MODE_DIR: u16 = 0x4000;

/// mode 高 4 位：常规文件。
const MODE_REG: u16 = 0x8000;

/// 目录项固定头：ino(4) + rec_len(2) + name_len(1) + file_type(1)。
const DIRENT_MIN: usize = 8;

/// 块大小（本驱动固定口径）。
const BS: u64 = 1024;

/// 内部错误码（非 UEFI 状态；串口以 16 进制直报）。
pub mod err {
    pub const BAD_MAGIC: usize = 0x21;
    pub const BAD_INO: usize = 0x23;
    pub const CORRUPT_SB: usize = 0x24;
    pub const CORRUPT_DIRENT: usize = 0x25;
    pub const BAD_BLOCK_SIZE: usize = 0x26;
    pub const INDIRECT2_UNSUPPORTED: usize = 0x27;
    pub const BAD_MODE: usize = 0x28;
    pub const BLOCK_RANGE: usize = 0x29;
}

/// 解析后的超级块（只读路径所需字段）。
pub struct Superblock {
    pub inodes_count: u32,
    pub blocks_count: u32,
    pub inodes_per_group: u32,
    pub inode_size: u16,
    pub first_data_block: u32,
}

/// inode 的只读视图。
pub struct Inode {
    pub mode: u16,
    pub size: u32,
    pub blocks: [u32; 15],
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & 0xF000 == MODE_DIR
    }

    pub fn is_reg(&self) -> bool {
        self.mode & 0xF000 == MODE_REG
    }
}

/// 间接表缓存（二级间接路径命中率最高：l2 每 256 块读一次、l1 每 256 块读一次）。
struct MapCache {
    l2_phys: u32,
    l2: [u8; 1024],
    l1_phys: u32,
    l1: [u8; 1024],
}

impl MapCache {
    const EMPTY: MapCache = MapCache { l2_phys: 0, l2: [0u8; 1024], l1_phys: 0, l1: [0u8; 1024] };

    fn load_l2(&mut self, dev: &mut dyn BlockRead, vol: &Volume, phys: u32) -> Result<(), usize> {
        if self.l2_phys != phys {
            vol.read_block(dev, phys, &mut self.l2)?;
            self.l2_phys = phys;
        }
        Ok(())
    }

    fn l2_entry(&self, idx: usize) -> u32 {
        let o = idx * 4;
        u32::from_le_bytes([self.l2[o], self.l2[o + 1], self.l2[o + 2], self.l2[o + 3]])
    }

    fn l1_of(&mut self, dev: &mut dyn BlockRead, vol: &Volume, phys: u32) -> Result<(), usize> {
        if self.l1_phys != phys {
            vol.read_block(dev, phys, &mut self.l1)?;
            self.l1_phys = phys;
        }
        Ok(())
    }

    fn l1_entry(&self, idx: usize) -> u32 {
        let o = idx * 4;
        u32::from_le_bytes([self.l1[o], self.l1[o + 1], self.l1[o + 2], self.l1[o + 3]])
    }
}
/// 已挂载卷。
pub struct Volume {
    sb: Superblock,
    /// 文件系统在设备内的起始字节（MBR 分区盘用；整盘挂载为 0）。
    base: u64,
}

/// 打开的文件。
pub struct File {
    inode: Inode,
}

impl File {
    /// 文件字节数（调用方据此分配读取缓冲）。
    pub fn size(&self) -> u32 {
        self.inode.size
    }
}

impl Volume {
    /// 挂载：读 SB@1024（绝对偏移——1024B 块的 SB 就在字节 1024），
    /// 魔数与几何 sanity 全过才返回卷。
    pub fn mount(dev: &mut dyn BlockRead) -> Result<Volume, usize> {
        Self::mount_at(dev, 0)
    }

    /// 在设备内 `base` 字节处挂载（MBR 分区：base = 分区起始 LBA × 512）。
    pub fn mount_at(dev: &mut dyn BlockRead, base: u64) -> Result<Volume, usize> {
        let mut sb_raw = [0u8; 1024];
        dev.read_at(base + 1024, &mut sb_raw)?;
        let magic = u16::from_le_bytes([sb_raw[56], sb_raw[57]]);
        if magic != EXT2_MAGIC {
            return Err(err::BAD_MAGIC);
        }
        let log_bs = u32::from_le_bytes([sb_raw[24], sb_raw[25], sb_raw[26], sb_raw[27]]);
        if log_bs != 0 {
            return Err(err::BAD_BLOCK_SIZE); // 只支持 1024（口径见模块头）
        }
        let fdb = u32::from_le_bytes([sb_raw[20], sb_raw[21], sb_raw[22], sb_raw[23]]);
        if fdb != 1 {
            // 规范：fdb==1 当且仅当块大小 1024
            return Err(err::CORRUPT_SB);
        }
        let inodes_count = u32::from_le_bytes([
            sb_raw[0], sb_raw[1], sb_raw[2], sb_raw[3],
        ]);
        let blocks_count = u32::from_le_bytes([
            sb_raw[4], sb_raw[5], sb_raw[6], sb_raw[7],
        ]);
        let ipg = u32::from_le_bytes([sb_raw[40], sb_raw[41], sb_raw[42], sb_raw[43]]);
        let inode_size = u16::from_le_bytes([sb_raw[88], sb_raw[89]]);
        if inodes_count == 0 || blocks_count == 0 || ipg == 0 || inode_size < 128 {
            return Err(err::CORRUPT_SB);
        }
        Ok(Volume {
            base,
            sb: Superblock {
                inodes_count,
                blocks_count,
                inodes_per_group: ipg,
                inode_size,
                first_data_block: fdb,
            },
        })
    }

    /// 按 inode 号读 inode：组 = (ino-1)/ipg，GDT 项 @GDT+组*32，
    /// inode 表块号在项偏移 8。
    fn read_inode(&self, dev: &mut dyn BlockRead, ino: u32) -> Result<Inode, usize> {
        if ino == 0 || ino > self.sb.inodes_count {
            return Err(err::BAD_INO);
        }
        let group = (ino - 1) / self.sb.inodes_per_group;
        let gdt_off = self.base + (self.sb.first_data_block as u64 + 1) * BS + group as u64 * 32;
        let mut gd = [0u8; 32];
        dev.read_at(gdt_off, &mut gd)?;
        let inode_table = u32::from_le_bytes([gd[8], gd[9], gd[10], gd[11]]);
        if inode_table == 0 {
            return Err(err::CORRUPT_SB);
        }
        let idx = (ino - 1) % self.sb.inodes_per_group;
        let abs = self.base + inode_table as u64 * BS + idx as u64 * self.sb.inode_size as u64;
        // inode 原始区只取前 100 字节（mode..i_block 数组止于 100）
        let mut raw = [0u8; 100];
        dev.read_at(abs, &mut raw)?;
        let mut blocks = [0u32; 15];
        for (i, slot) in blocks.iter_mut().enumerate() {
            let o = INODE_BLOCK_ARRAY + i * 4;
            *slot = u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
        }
        Ok(Inode {
            mode: u16::from_le_bytes([raw[0], raw[1]]),
            size: u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
            blocks,
        })
    }

    /// 逻辑块号 → 物理块号：直块 12 + 一级间接 256 + 二级间接 256²（每块
    /// 1024B 块 256 个 u32 指针）。超出即 INDIRECT2_UNSUPPORTED（> 64.3MiB）。
    ///
    /// `cache` 缓存最近用到的间接表，避免每个数据块都重读指针块（真实内核
    /// 24.6MB 在 1KiB 块下是 24576 次映射——无缓存会多出数万次 BlockIo）。
    fn map_logical(
        &self,
        dev: &mut dyn BlockRead,
        inode: &Inode,
        logical: u32,
        cache: &mut MapCache,
    ) -> Result<u32, usize> {
        if (logical as usize) < 12 {
            return Ok(inode.blocks[logical as usize]);
        }
        let rem = logical - 12;
        if rem < 256 {
            // 一级间接
            let l1 = inode.blocks[12];
            if l1 == 0 {
                return Ok(0); // 稀疏
            }
            cache.l1_of(dev, self, l1)?;
            return Ok(cache.l1_entry(rem as usize));
        }
        let rem2 = rem - 256;
        const PER_BLOCK: u32 = 256;
        if rem2 >= PER_BLOCK * PER_BLOCK {
            return Err(err::INDIRECT2_UNSUPPORTED);
        }
        let l2 = inode.blocks[13];
        if l2 == 0 {
            return Ok(0); // 稀疏
        }
        cache.load_l2(dev, self, l2)?;
        let l1 = cache.l2_entry((rem2 / PER_BLOCK) as usize);
        if l1 == 0 {
            return Ok(0);
        }
        cache.l1_of(dev, self, l1)?;
        Ok(cache.l1_entry((rem2 % PER_BLOCK) as usize))
    }

    /// 连续物理块一次读完（len 为 1024 的倍数）：省掉每块一次固件往返。
    fn read_block_run(&self, dev: &mut dyn BlockRead, phys: u32, buf: &mut [u8]) -> Result<(), usize> {
        let n = buf.len() as u64;
        if phys == 0 || phys as u64 + n / BS > self.sb.blocks_count as u64 {
            return Err(err::BLOCK_RANGE);
        }
        dev.read_at(self.base + phys as u64 * BS, buf)
    }

    fn read_block(&self, dev: &mut dyn BlockRead, phys: u32, buf: &mut [u8]) -> Result<(), usize> {
        if phys == 0 || phys as u64 >= self.sb.blocks_count as u64 {
            return Err(err::BLOCK_RANGE);
        }
        dev.read_at(self.base + phys as u64 * BS, &mut buf[..1024])
    }

    /// 读 inode 数据到 buf，返回实际读取字节数（EOF 截断；稀疏洞读零）。
    pub fn read_data(
        &self,
        dev: &mut dyn BlockRead,
        inode: &Inode,
        buf: &mut [u8],
    ) -> Result<usize, usize> {
        let want = core::cmp::min(buf.len(), inode.size as usize);
        let mut done = 0usize;
        let mut scratch = [0u8; 1024];
        let mut cache = MapCache::EMPTY;
        while done < want {
            let logical = (done / 1024) as u32;
            let in_block = done % 1024;
            let phys = self.map_logical(dev, inode, logical, &mut cache)?;
            if phys == 0 {
                let take = core::cmp::min(want - done, 1024 - in_block);
                for b in &mut buf[done..done + take] {
                    *b = 0;
                }
                done += take;
                continue;
            }
            // 合并**连续物理块**：一次 BlockIo 读一整段。
            // 此前每 1KiB 一次读（release 内核 24MB → 2.4 万次固件往返，TCG 下数分钟）；
            // 连续段读取把调用次数降一到两个数量级（Limine 同思路）。仅在块边界起合并
            // （非对齐首块走下面的单块路径），上限 31 块 = 31KiB：read_at 的 span 会多算
            // 一块，31 块刚好一次调用读完（32 块×设备块大小 ≤ 既有 ATAPI 上限）。
            let mut run = 1usize;
            if in_block == 0 {
                const MAX_RUN_BLOCKS: usize = 31;
                while run < MAX_RUN_BLOCKS {
                    if (done + run * 1024) >= want {
                        break;
                    }
                    let next = self.map_logical(dev, inode, logical + run as u32, &mut cache)?;
                    if next != phys + run as u32 {
                        break;
                    }
                    run += 1;
                }
            }
            if run > 1 {
                let run_bytes = core::cmp::min(run * 1024, want - done);
                self.read_block_run(dev, phys, &mut buf[done..done + run_bytes])?;
                done += run_bytes;
                continue;
            }
            let take = core::cmp::min(want - done, 1024 - in_block);
            self.read_block(dev, phys, &mut scratch)?;
            buf[done..done + take].copy_from_slice(&scratch[in_block..in_block + take]);
            done += take;
        }
        Ok(done)
    }

    /// 目录内查找名字，返回目标 inode 号。目录数据上限一个块
    /// （1024B：fixture 与安装镜像的根/BOOT 目录口径，超出即报错）。
    fn dir_lookup(&self, dev: &mut dyn BlockRead, dir: &Inode, name: &[u8]) -> Result<u32, usize> {
        if !dir.is_dir() {
            return Err(err::BAD_MODE);
        }
        if dir.size as usize > 1024 {
            return Err(err::CORRUPT_DIRENT);
        }
        let mut data = [0u8; 1024];
        let n = self.read_data(dev, dir, &mut data)?;
        let d = &data[..n];
        let mut cur = 0usize;
        while cur + DIRENT_MIN <= n {
            let ino = u32::from_le_bytes([d[cur], d[cur + 1], d[cur + 2], d[cur + 3]]);
            let rec = u16::from_le_bytes([d[cur + 4], d[cur + 5]]) as usize;
            let name_len = d[cur + 6] as usize;
            if rec < DIRENT_MIN || cur + rec > n {
                return Err(err::CORRUPT_DIRENT);
            }
            if ino != 0 && name_len > 0 {
                if DIRENT_MIN + name_len > rec {
                    return Err(err::CORRUPT_DIRENT);
                }
                if &d[cur + DIRENT_MIN..cur + DIRENT_MIN + name_len] == name {
                    return Ok(ino);
                }
            }
            cur += rec;
        }
        Err(efi::EFI_NOT_FOUND)
    }

    /// 路径下钻：EXT2 语义大小写敏感；中间节点必须是目录，末节点必须是常规文件。
    pub fn open_path(&self, dev: &mut dyn BlockRead, path: &str) -> Result<File, usize> {
        let mut cur = self.read_inode(dev, ROOT_INO)?;
        let mut iter = Components::new(path);
        let mut resolved_any = false;
        while let Some(comp) = iter.peek() {
            let is_last = !iter.has_more_after_current();
            let ino = self.dir_lookup(dev, &cur, comp)?;
            cur = self.read_inode(dev, ino)?;
            if !is_last && !cur.is_dir() {
                return Err(err::BAD_MODE);
            }
            resolved_any = true;
            iter.advance();
        }
        if !resolved_any {
            return Err(efi::EFI_NOT_FOUND); // 空路径无文件语义
        }
        if !cur.is_reg() {
            return Err(err::BAD_MODE); // 末节点不是常规文件
        }
        Ok(File { inode: cur })
    }

    /// 便捷读：缓冲过小返回 Err(所需大小)。
    pub fn read_file(&self, dev: &mut dyn BlockRead, f: &File, buf: &mut [u8]) -> Result<usize, usize> {
        if buf.len() < f.inode.size as usize {
            return Err(f.inode.size as usize);
        }
        self.read_data(dev, &f.inode, buf)
    }
}

/// 路径组件迭代器（无 alloc；斜杠分隔；空组件跳过）。
struct Components<'a> {
    rest: &'a str,
    cur: Option<&'a str>,
}

impl<'a> Components<'a> {
    fn new(path: &'a str) -> Self {
        let mut it = Components { rest: path, cur: None };
        it.fill();
        it
    }

    /// 填充 cur 为下一个非空组件。
    fn fill(&mut self) {
        self.cur = None;
        while !self.rest.is_empty() {
            let (head, tail) = match self.rest.split_once('/') {
                Some((h, t)) => (h, t),
                None => (self.rest, ""),
            };
            self.rest = tail;
            if !head.is_empty() {
                self.cur = Some(head);
                return;
            }
        }
    }

    fn peek(&self) -> Option<&'a [u8]> {
        self.cur.map(|c| c.as_bytes())
    }

    /// cur 之后是否还有组件（判定 is_last）。
    fn has_more_after_current(&self) -> bool {
        // rest 非空即还有内容；rest 只剩空组件时也视为无更多。
        !self.rest.is_empty()
    }

    fn advance(&mut self) {
        self.fill();
    }
}