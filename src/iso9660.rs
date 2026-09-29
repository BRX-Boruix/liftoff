//! ISO9660 只读解析器（ECMA-119 基本卷，无扩展）。
//!
//! 纯解析器：块读取经 [`BlockRead`] trait 注入，本模块不触碰 UEFI——
//! 单元级验证用内存块设备，集成验证走 QEMU。M2c 的 EXT2 复用同一注入模式。
//!
//! 已知限制（对比 brxLimine 的 iso9660.s2.c）：不解析 Rock Ridge / Joliet；
//! 不支持 multi-extent（内核 ELF 单 extent 上限 4GB，超出即报错）；
//! 目录大小上限 64MiB（防损毁镜像的荒谬值）。

use crate::efi;

/// 逻辑扇区大小（ECMA-119 6.1.2 / 8.4.10）。
pub const SECTOR: usize = 2048;

/// 目录/元数据缓冲上限：防损毁镜像。
const MAX_DIR_SIZE: u32 = 64 * 1024 * 1024;

/// PVD 扫描起点（8.4：卷描述符序列从 LBA 16 开始）。
const FIRST_VD_LBA: u64 = 16;

/// PVD 扫描上限（超过即认为损毁）。
const MAX_VD_SCAN: u64 = 256;

/// 目录记录标志位：目录（9.1.6）。
const FLAG_DIR: u8 = 0x02;

/// 块读取抽象。usize 级错误由实现者翻译。
pub trait BlockRead {
    /// 从字节偏移 `off` 读 `buf.len()` 字节。部分读即错误。
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<(), usize>;
}

/// UEFI BlockIo 适配器：媒体句柄与块大小来自固件。
pub struct UefiBlock<'a> {
    bs: &'a efi::BootServices,
    bio: &'a efi::BlockIo,
    media_id: u32,
}

impl<'a> UefiBlock<'a> {
    pub fn new(bs: &'a efi::BootServices, bio: &'a efi::BlockIo) -> Self {
        // SAFETY: media 指针由固件在协议内保证有效（至 ExitBootServices）。
        let media_id = unsafe { (*bio.media).media_id };
        UefiBlock { bs, bio, media_id }
    }
}

impl BlockRead for UefiBlock<'_> {
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> Result<(), usize> {
        // BlockIo 只按块读：把字节区间扩展到块边界。
        let bs = unsafe { (*self.bio.media).block_size } as u64;
        if bs == 0 {
            return Err(0x10); // 本模块内部错误码：块大小非法
        }
        let first = off / bs;
        let last = (off + buf.len() as u64 + bs - 1) / bs; // 不含
        let blocks = (last - first) as usize;
        if blocks == 0 {
            return Ok(());
        }
        // 大读拆窗（≤32 块/次，64KB@2KB 块）：OVMF 的 ATAPI DMA 对超大单次
        // 传输会挂起——M4 boot 变体读 4.3MB 内核时实测（240s 无返回）。
        const MAX_BLOCKS_PER_CALL: usize = 32;
        let mut done = 0usize; // 已复制的字节数
        let mut block = first;
        while done < buf.len() {
            let want = buf.len() - done;
            // 尾窗不越过媒体最后一块（read_blocks 越界返回 INVALID_PARAMETER）。
            let last_block = unsafe { (*self.bio.media).last_block };
            let avail = (last_block as usize).saturating_sub(block as usize) + 1;
            if avail == 0 || block > last_block {
                return Err(0x13);
            }
            let span_blocks = (((want + bs as usize - 1) / bs as usize) + 1)
                .min(MAX_BLOCKS_PER_CALL)
                .min(avail);
            let span_bytes = span_blocks * bs as usize;
            if span_blocks == 0 {
                return Err(0x13); // 本模块内部码：读到设备末尾之外
            }
            let mut tmp = PoolBuf::new(self.bs, span_bytes)?;
            // SAFETY: tmp 缓冲来自本次分配，read_blocks 按块填入。
            let status = unsafe {
                (self.bio.read_blocks)(self.bio, self.media_id, block, span_bytes, tmp.as_slice().as_mut_ptr())
            };
            if efi::is_error(status) {
                return Err(status);
            }
            // 本窗口内属于 buf 的字节区间：首块可能有 skip（仅首窗）。
            let src_off = if done == 0 { (off - first * bs) as usize } else { 0 };
            let take = (span_bytes - src_off).min(want);
            buf[done..done + take].copy_from_slice(&tmp.as_slice()[src_off..src_off + take]);
            done += take;
            block += span_blocks as u64;
        }
        Ok(())
    }
}

/// 固件池缓冲守卫（S18/S15）：分配走 BootServices.allocate_pool，Drop 即 free_pool。
/// 单点定义——本工程的暂存缓冲只有这一条路径，M4 的 memmap 缓冲同样复用。
pub struct PoolBuf<'a> {
    bs: &'a efi::BootServices,
    ptr: *mut u8,
    len: usize,
}

impl<'a> PoolBuf<'a> {
    /// EFI_BOOT_SERVICES_DATA：仅引导阶段存活的暂存数据。
    const POOL_TYPE: u32 = 4;

    pub fn new(bs: &'a efi::BootServices, len: usize) -> Result<PoolBuf<'a>, usize> {
        let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
        // SAFETY: 固件调用；失败时 raw 保持空。
        let st = unsafe { (bs.allocate_pool)(Self::POOL_TYPE, len, &mut raw) };
        if efi::is_error(st) {
            return Err(st);
        }
        Ok(PoolBuf { bs, ptr: raw as *mut u8, len })
    }

    pub fn as_slice(&mut self) -> &mut [u8] {
        // SAFETY: 分配 len 字节成功后 ptr 有效。
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl core::ops::Deref for PoolBuf<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: 分配成功后 ptr/len 恒有效。
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl core::ops::DerefMut for PoolBuf<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: 同上，独占借用。
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for PoolBuf<'_> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: 本缓冲生命周期内指针未变；释放失败无可恢复路径（M4 重审）。
            unsafe { (self.bs.free_pool)(self.ptr as *mut core::ffi::c_void) };
        }
    }
}

/// 已挂载卷：根目录的定位与大小。
pub struct Volume {
    root_lba: u64,
    root_size: u32,
}

/// 打开的文件：数据的 LBA 与字节数（单 extent）。
pub struct File {
    pub lba: u64,
    pub size: u32,
}

/// 目录记录的解析视图。借用原始缓冲，生命周期由调用方管。
struct Rec<'a> {
    lba: u64,
    size: u32,
    flags: u8,
    name: &'a [u8],
}

/// 从 buf[off..] 解析一条记录。空间不足/畸形返回 None。
fn parse_rec(buf: &[u8], off: usize) -> Option<Rec<'_>> {
    if off >= buf.len() {
        return None;
    }
    let len = buf[off] as usize;
    if len == 0 {
        return None; // 调用方负责跳扇区
    }
    if len < 33 || off + len > buf.len() {
        return None;
    }
    let e = &buf[off..off + len];
    let lba = u32::from_le_bytes([e[2], e[3], e[4], e[5]]) as u64;
    let size = u32::from_le_bytes([e[10], e[11], e[12], e[13]]);
    let flags = e[25];
    let name_len = e[32] as usize;
    if 33 + name_len > len {
        return None;
    }
    let name = &e[33..33 + name_len];
    // 名字可能带一个填充字节（记录总长偶数化后名字后多 1 字节，9.1.12）
    if name_len > 0 && (33 + name_len) < len && name[name_len - 1] == 0 {
        // 仅当规范允许的填充位（名字长度奇偶规则）——直接截掉 NUL 是安全的：
        // 8.4.18/9.1.12 名字后填充不属于名字。
    }
    Some(Rec { lba, size, flags, name })
}

/// 归一化名字比较：去掉 ";1" 后缀与尾部 '.'，ASCII 大小写不敏感。
/// （未来引入 xorriso 生成的真实 ISO 时，Rock Ridge 名走同一路径也无碍：
/// 它们不含 ";1"，只是大小写敏感度略宽，不会误配 8.3 名。）
fn name_matches(entry: &[u8], want: &[u8]) -> bool {
    let base = strip_version(entry);
    if base.len() != want.len() {
        return false;
    }
    for (a, b) in base.iter().zip(want.iter()) {
        if a.to_ascii_lowercase() != b.to_ascii_lowercase() {
            return false;
        }
    }
    true
}

/// 去掉 ";版本号" 后缀。
fn strip_version(name: &[u8]) -> &[u8] {
    if let Some(pos) = name.iter().position(|&c| c == b';') {
        let mut end = pos;
        // 顺带去掉版本分隔符前的尾点（"NAME.;1" → "NAME"）
        if end > 0 && name[end - 1] == b'.' {
            end -= 1;
        }
        &name[..end]
    } else {
        name
    }
}

/// 目录缓冲内线性查找。跨扇区跳转按规范处理：记录不跨扇区，
/// length==0 表示本扇区剩余空间填充，跳到下一扇区继续。
fn find_in_dir<'a>(
    dir: &'a [u8],
    component: &[u8],
    want_dir: bool,
) -> Option<Rec<'a>> {
    let mut off = 0;
    while off < dir.len() {
        if dir[off] == 0 {
            // 跳到下一扇区边界
            let next = (off / SECTOR + 1) * SECTOR;
            if next >= dir.len() {
                return None;
            }
            off = next;
            continue;
        }
        let rec = parse_rec(dir, off)?;
        let step = dir[off] as usize;
        if name_matches(rec.name, component) {
            let is_dir = rec.flags & FLAG_DIR != 0;
            if is_dir == want_dir {
                return Some(rec);
            }
            return None; // 类型不符即不存在（不继续扫：名字唯一）
        }
        off += step;
    }
    None
}

impl Volume {
    /// 探测并挂载：扫描 PVD，校验 "CD001" 与根目录尺寸。
    pub fn mount(bs: &efi::BootServices, dev: &mut dyn BlockRead) -> Result<Volume, usize> {
        let mut lba = FIRST_VD_LBA;
        let end = FIRST_VD_LBA + MAX_VD_SCAN;
        while lba < end {
            let mut sec = PoolBuf::new(bs, SECTOR)?;
            dev.read_at(lba * SECTOR as u64, sec.as_slice())?;
            match sec[0] {
                1 => {
                    if &sec[1..6] != b"CD001" {
                        return Err(0x11); // 不是 ISO9660
                    }
                    // 根记录嵌在 PVD 偏移 156（8.4.18），字段偏移与普通记录一致
                    let p = &sec[156..190];
                    let root_lba = u32::from_le_bytes([p[2], p[3], p[4], p[5]]) as u64;
                    let root_size = u32::from_le_bytes([p[10], p[11], p[12], p[13]]);
                    if root_size == 0 || root_size > MAX_DIR_SIZE || root_size as usize % SECTOR != 0 {
                        return Err(0x12); // 根目录尺寸非法
                    }
                    return Ok(Volume { root_lba, root_size });
                }
                255 => return Err(0x11), // 先于 PVD 的终结符：不是有效卷
                _ => lba += 1, // boot/supplementary/partition 描述符：继续扫
            }
        }
        Err(0x13) // PVD 扫描越限
    }

    /// 读整个目录到缓冲。
    fn read_dir<'a>(
        &self,
        bs: &'a efi::BootServices,
        dev: &mut dyn BlockRead,
        lba: u64,
        size: u32,
    ) -> Result<PoolBuf<'a>, usize> {
        let mut buf = PoolBuf::new(bs, size as usize)?;
        dev.read_at(lba * SECTOR as u64, buf.as_slice())?;
        Ok(buf)
    }

    /// 按路径打开。path 以 '/' 分隔；中间组件必须是目录，
    /// 末组件可为目录（返回其定位）或文件。
    pub fn open_path(
        &self,
        bs: &efi::BootServices,
        dev: &mut dyn BlockRead,
        path: &str,
    ) -> Result<File, usize> {
        // 组件切分：空组件（连续斜杠）跳过；全空 = 根目录，无文件语义，报错。
        let mut comps: Vec<&str, 8> = Vec::new();
        for c in path.split('/') {
            if c.is_empty() {
                continue;
            }
            if comps.push(c).is_err() {
                return Err(0x14); // 路径组件数超上限
            }
        }
        if comps.is_empty() {
            return Err(efi::EFI_NOT_FOUND);
        }

        let mut dir_lba = self.root_lba;
        let mut dir_size = self.root_size;
        let last = comps.len() - 1;
        for (i, comp) in comps.iter().enumerate() {
            if comp.len() == 0 || comp.len() > 207 {
                return Err(0x14); // 组件名长度出格（207 = 240 - 33 名字上限余量）
            }
            let mut dir = self.read_dir(bs, dev, dir_lba, dir_size)?;
            let is_last = i == last;
            // 中间组件必须是目录，末组件按文件找（目录匹配交给上层语义）。
            let rec = find_in_dir(dir.as_slice(), comp.as_bytes(), !is_last)
                .ok_or(efi::EFI_NOT_FOUND)?;
            if is_last {
                // multi-extent 不支持：续接记录会共享名字，此处只取第一段，
                // 若 flags 带 0x80 说明文件被切分，读出的内容不完整 → 拒绝。
                if rec.flags & 0x80 != 0 {
                    return Err(0x15);
                }
                return Ok(File { lba: rec.lba, size: rec.size });
            }
            dir_lba = rec.lba;
            dir_size = rec.size;
            if dir_size == 0 || dir_size > MAX_DIR_SIZE || dir_size as usize % SECTOR != 0 {
                return Err(0x12);
            }
        }
        Err(efi::EFI_NOT_FOUND)
    }

    /// 读文件全部内容到调用方缓冲。缓冲过小返回 Err(len)。
    pub fn read_file(
        &self,
        dev: &mut dyn BlockRead,
        f: &File,
        buf: &mut [u8],
    ) -> Result<usize, usize> {
        if buf.len() < f.size as usize {
            return Err(f.size as usize);
        }
        dev.read_at(f.lba * SECTOR as u64, &mut buf[..f.size as usize])?;
        Ok(f.size as usize)
    }
}

/// 定长栈向量（无 alloc）。容量 8 深的路径组件足够内核路径。
pub struct Vec<T, const N: usize> {
    buf: [Option<T>; N],
    len: usize,
}

impl<T, const N: usize> Vec<T, N> {
    pub fn new() -> Self {
        Vec { buf: core::array::from_fn(|_| None), len: 0 }
    }

    pub fn push(&mut self, v: T) -> Result<(), T> {
        if self.len >= N {
            return Err(v);
        }
        self.buf[self.len] = Some(v);
        self.len += 1;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.buf[..self.len].iter().map(|o| o.as_ref().unwrap())
    }
}