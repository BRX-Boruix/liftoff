//! EXT2 只读：超级块解析（L4）。
//!
//! 布局：超级块在**偏移 1024**、长 1024 字节；字段全部**小端**。
//! `s_log_block_size`（@24）是**移位量**：块大小 = `1024 << log`，规范上限 64 KiB（log ≤ 6），
//! 故**移位前先判上限**，避免 `1024 << 7` 这类越界值。
//! revision 0（`s_rev_level` @76 == 0）的超级块没有 `s_inode_size` 字段，规范规定 inode
//! 大小为 128，故按此处理（而不是读一个不存在的字段）。

/// EXT2 魔数（`s_magic` @56）。
pub const EXT2_MAGIC: u16 = 0xEF53;
/// 超级块偏移。
pub const EXT2_SUPERBLOCK_OFFSET: usize = 1024;
/// 超级块长度。
pub const EXT2_SUPERBLOCK_SIZE: usize = 1024;
/// 块大小移位上限：`1024 << 6 = 65536`（64 KiB）。
const MAX_LOG_BLOCK_SIZE: u32 = 6;
/// revision 0 的 inode 大小。
const REV0_INODE_SIZE: u16 = 128;

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ext2Error {
    /// 映像不足。
    ShortImage,
    /// 魔数不是 `0xEF53`。
    NotExt2,
    /// `s_log_block_size` 超出上限。
    BadBlockSize,
    /// inode 号非法（EXT2 的 inode 号从 **1** 起算）。
    BadInodeNumber,
    /// 目录项的 `rec_len` 非法（为 0 会死循环，越出块尾会越界读）。
    BadRecLen,
    /// 目录项的 `name_len` 超出其记录范围。
    BadNameLen,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
}

/// 超级块关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Superblock {
    /// inode 总数。
    pub inodes_count: u32,
    /// 块总数。
    pub blocks_count: u32,
    /// 首个数据块。
    pub first_data_block: u32,
    /// 块大小（字节）。
    pub block_size: u32,
    /// 每块组的 inode 数。
    pub inodes_per_group: u32,
    /// 首个可用 inode。
    pub first_ino: u32,
    /// inode 大小（字节）。
    pub inode_size: u16,
}

fn read_u16(raw: &[u8], at: usize) -> Option<u16> {
    let bytes = raw.get(at..at.checked_add(2)?)?;
    let mut buf = [0u8; 2];
    buf.copy_from_slice(bytes);
    Some(u16::from_le_bytes(buf))
}

fn read_u32(raw: &[u8], at: usize) -> Option<u32> {
    let bytes = raw.get(at..at.checked_add(4)?)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    Some(u32::from_le_bytes(buf))
}

/// 解析 EXT2 超级块。
pub fn parse_superblock(image: &[u8]) -> Result<Superblock, Ext2Error> {
    let end = EXT2_SUPERBLOCK_OFFSET
        .checked_add(EXT2_SUPERBLOCK_SIZE)
        .ok_or(Ext2Error::ShortImage)?;
    let raw = image
        .get(EXT2_SUPERBLOCK_OFFSET..end)
        .ok_or(Ext2Error::ShortImage)?;
    if read_u16(raw, 56).ok_or(Ext2Error::ShortImage)? != EXT2_MAGIC {
        return Err(Ext2Error::NotExt2);
    }
    let log_block_size = read_u32(raw, 24).ok_or(Ext2Error::ShortImage)?;
    if log_block_size > MAX_LOG_BLOCK_SIZE {
        return Err(Ext2Error::BadBlockSize);
    }
    let revision = read_u32(raw, 76).ok_or(Ext2Error::ShortImage)?;
    let inode_size = if revision == 0 {
        REV0_INODE_SIZE
    } else {
        read_u16(raw, 88).ok_or(Ext2Error::ShortImage)?
    };
    Ok(Superblock {
        inodes_count: read_u32(raw, 0).ok_or(Ext2Error::ShortImage)?,
        blocks_count: read_u32(raw, 4).ok_or(Ext2Error::ShortImage)?,
        first_data_block: read_u32(raw, 20).ok_or(Ext2Error::ShortImage)?,
        block_size: 1024u32 << log_block_size,
        inodes_per_group: read_u32(raw, 40).ok_or(Ext2Error::ShortImage)?,
        first_ino: read_u32(raw, 84).ok_or(Ext2Error::ShortImage)?,
        inode_size,
    })
}

/// 目录项（`ext2_dir_entry`：inode@0、rec_len@4、name_len@6、file_type@7、name@8）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirEntry {
    /// inode 号（0 表示已删除）。
    pub inode: u32,
    /// 名字长度。
    pub name_len: u8,
    /// 文件类型。
    pub file_type: u8,
    /// 名字（定长存放，避免分配；有效范围是前 `name_len` 字节）。
    pub name: [u8; 255],
}

impl DirEntry {
    /// 占位值。
    pub const EMPTY: Self = Self { inode: 0, name_len: 0, file_type: 0, name: [0; 255] };

    /// 名字切片。
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

/// 取 `at` 处的下一个目录项，返回条目与**下一个偏移**；`at` 越出块尾返回 `None`。
///
/// 这是目录遍历的**单点定义**（`parse_dir_entries` 与 `find_in_dir` 都用它）。
/// 数值边界：`rec_len == 0` 会让游标原地不动（**死循环**），故立即报错；
/// `at + rec_len` 越出块尾同样报错（不越界读）；`name_len` 必须落在 `rec_len - 8` 之内。
/// `inode == 0` 表示已删除：**返回该条目**，由调用方决定是否跳过（但游标一定前进）。
pub fn next_dir_entry(block: &[u8], at: usize) -> Result<Option<(DirEntry, usize)>, Ext2Error> {
    if at.checked_add(8).ok_or(Ext2Error::BadRecLen)? > block.len() {
        return Ok(None);
    }
    let inode = read_u32(block, at).ok_or(Ext2Error::ShortImage)?;
    let rec_len = read_u16(block, at + 4).ok_or(Ext2Error::ShortImage)? as usize;
    let name_len = *block.get(at + 6).ok_or(Ext2Error::ShortImage)? as usize;
    let file_type = *block.get(at + 7).ok_or(Ext2Error::ShortImage)?;
    if rec_len == 0 {
        return Err(Ext2Error::BadRecLen);
    }
    let end = at.checked_add(rec_len).ok_or(Ext2Error::BadRecLen)?;
    if end > block.len() {
        return Err(Ext2Error::BadRecLen);
    }
    if name_len > rec_len - 8 {
        return Err(Ext2Error::BadNameLen);
    }
    let mut entry = DirEntry::EMPTY;
    entry.inode = inode;
    entry.name_len = name_len as u8;
    entry.file_type = file_type;
    entry.name[..name_len].copy_from_slice(&block[at + 8..at + 8 + name_len]);
    Ok(Some((entry, end)))
}

/// 解析一个目录块里的条目，返回条目数（跳过已删除项）。
///
/// 调用方给的缓冲决定上限：超出即报 `BufferTooSmall`（**不静默截断**）。
pub fn parse_dir_entries(block: &[u8], out: &mut [DirEntry]) -> Result<usize, Ext2Error> {
    let mut count = 0;
    let mut at = 0usize;
    while let Some((entry, next)) = next_dir_entry(block, at)? {
        if entry.inode != 0 {
            if count == out.len() {
                return Err(Ext2Error::BufferTooSmall);
            }
            out[count] = entry;
            count += 1;
        }
        at = next;
    }
    Ok(count)
}

/// 在目录块里按名字查找（**大小写敏感**、精确长度匹配）。
///
/// **流式**遍历（O(1) 额外内存），因为 `DirEntry` 带 255 字节定长名字，物化整块条目会
/// 在引导器的栈上炸掉。目录本身损坏的错误照原样向上返回，**不吞成“找不到”**
/// （否则会把坏盘误判成文件不存在）。已删除项（`inode == 0`）不参与匹配。
pub fn find_in_dir(block: &[u8], name: &[u8]) -> Result<Option<DirEntry>, Ext2Error> {
    let mut at = 0usize;
    while let Some((entry, next)) = next_dir_entry(block, at)? {
        if entry.inode != 0 && entry.name() == name {
            return Ok(Some(entry));
        }
        at = next;
    }
    Ok(None)
}

/// EXT2 inode 的关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Inode {
    /// `i_mode`（@0）。
    pub mode: u16,
    /// `i_size` 的低 32 位（@4）。
    pub size: u32,
    /// `i_block[15]`（@40）：前 12 个直接块，[12] 一级间接、[13] 二级、[14] 三级。
    pub blocks: [u32; 15],
}

/// 解析 inode 表里的第 `index` 个 inode（**1 起算**）。
///
/// 数值边界：先拒绝 0（否则 `index - 1` 会下溢），再用 checked 运算算偏移；
/// 偏移与读取范围都必须落在表内。
pub fn parse_inode(table: &[u8], index: u32, inode_size: u16) -> Result<Inode, Ext2Error> {
    if index == 0 {
        return Err(Ext2Error::BadInodeNumber);
    }
    let size = inode_size as usize;
    let at = (index as usize - 1)
        .checked_mul(size)
        .ok_or(Ext2Error::ShortImage)?;
    let end = at.checked_add(size).ok_or(Ext2Error::ShortImage)?;
    let raw = table.get(at..end).ok_or(Ext2Error::ShortImage)?;
    let mut blocks = [0u32; 15];
    for (slot, value) in blocks.iter_mut().enumerate() {
        *value = read_u32(raw, 40 + slot * 4).ok_or(Ext2Error::ShortImage)?;
    }
    Ok(Inode {
        mode: read_u16(raw, 0).ok_or(Ext2Error::ShortImage)?,
        size: read_u32(raw, 4).ok_or(Ext2Error::ShortImage)?,
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::{EXT2_MAGIC, EXT2_SUPERBLOCK_OFFSET, Ext2Error, Superblock, parse_superblock};
    use std::vec::Vec;

    /// 造一个映像：1024 字节填充 + 1024 字节超级块。
    fn image(log_block_size: u32, magic: u16) -> Vec<u8> {
        let mut raw = std::vec![0u8; EXT2_SUPERBLOCK_OFFSET + 1024];
        let at = EXT2_SUPERBLOCK_OFFSET;
        raw[at..at + 4].copy_from_slice(&1024u32.to_le_bytes());
        raw[at + 4..at + 8].copy_from_slice(&8192u32.to_le_bytes());
        raw[at + 20..at + 24].copy_from_slice(&1u32.to_le_bytes());
        raw[at + 24..at + 28].copy_from_slice(&log_block_size.to_le_bytes());
        raw[at + 40..at + 44].copy_from_slice(&128u32.to_le_bytes());
        raw[at + 56..at + 58].copy_from_slice(&magic.to_le_bytes());
        raw[at + 76..at + 80].copy_from_slice(&1u32.to_le_bytes());
        raw[at + 84..at + 88].copy_from_slice(&11u32.to_le_bytes());
        raw[at + 88..at + 90].copy_from_slice(&128u16.to_le_bytes());
        raw
    }

    #[test]
    fn the_real_image_parses_with_the_values_verified_by_hand() {
        // 夹具由 mke2fs（Android 构建）生成，字段值已用十六进制手工核实：
        // 魔数 0xEF53、块大小 1024、inode_size 128、blocks_count 256、rev_level 1。
        let raw = std::fs::read("tests/fixtures/hello.ext2").expect("夹具镜像应存在");
        assert_eq!(raw.len(), 262144, "夹具应为 256 KiB");
        let sb = parse_superblock(&raw).expect("真实镜像应可解析");
        assert_eq!(sb.block_size, 1024);
        assert_eq!(sb.inode_size, 128);
        assert_eq!(sb.blocks_count, 256);
    }

    #[test]
    fn a_minimal_image_yields_its_fields() {
        let raw = image(0, EXT2_MAGIC);
        let sb: Superblock = parse_superblock(&raw).expect("超级块有效");
        assert_eq!(sb.block_size, 1024, "1024 << 0");
        assert_eq!(sb.inodes_count, 1024);
        assert_eq!(sb.blocks_count, 8192);
        assert_eq!(sb.inodes_per_group, 128);
        assert_eq!(sb.first_ino, 11);
        assert_eq!(sb.inode_size, 128);
    }

    #[test]
    fn a_four_kib_block_size_is_computed_from_the_log() {
        let raw = image(2, EXT2_MAGIC);
        let sb = parse_superblock(&raw).expect("超级块有效");
        assert_eq!(sb.block_size, 4096, "1024 << 2");
    }

    #[test]
    fn a_bad_magic_is_rejected() {
        let raw = image(0, 0x1234);
        assert_eq!(parse_superblock(&raw), Err(Ext2Error::NotExt2));
    }

    #[test]
    fn an_oversized_log_block_size_is_rejected() {
        // 1024 << 7 = 131072，超出 EXT2 的块大小上限（64 KiB）。
        let raw = image(7, EXT2_MAGIC);
        assert_eq!(parse_superblock(&raw), Err(Ext2Error::BadBlockSize));
    }

    #[test]
    fn a_short_image_is_rejected() {
        let raw = std::vec![0u8; 512];
        assert_eq!(parse_superblock(&raw), Err(Ext2Error::ShortImage));
    }
}

#[cfg(test)]
mod dir_tests {
    use super::{DirEntry, Ext2Error, parse_dir_entries};
    use std::vec::Vec;

    /// 造一个目录块：按顺序写入 (inode, name, file_type) 条目，最后一项吃掉剩余空间。
    fn dir_block(items: &[(u32, &str, u8)]) -> Vec<u8> {
        let mut block = std::vec![0u8; 1024];
        let mut at = 0usize;
        for (index, (inode, name, kind)) in items.iter().enumerate() {
            let need = 8 + name.len();
            let rec_len = if index + 1 == items.len() {
                1024 - at
            } else {
                (need + 3) & !3
            };
            block[at..at + 4].copy_from_slice(&inode.to_le_bytes());
            block[at + 4..at + 6].copy_from_slice(&(rec_len as u16).to_le_bytes());
            block[at + 6] = name.len() as u8;
            block[at + 7] = *kind;
            block[at + 8..at + 8 + name.len()].copy_from_slice(name.as_bytes());
            at += rec_len;
        }
        block
    }

    fn empty_out() -> [DirEntry; 8] {
        [DirEntry::EMPTY; 8]
    }

    #[test]
    fn entries_are_walked_by_rec_len() {
        let block = dir_block(&[(2, ".", 2), (2, "..", 2), (11, "HELLO.TXT", 1)]);
        let mut out = empty_out();
        let count = parse_dir_entries(&block, &mut out).expect("解析成功");
        assert_eq!(count, 3);
        assert_eq!(out[0].inode, 2);
        assert_eq!(out[2].inode, 11);
        assert_eq!(out[2].name_len, 9);
        assert_eq!(&out[2].name[..9], b"HELLO.TXT");
        assert_eq!(out[2].file_type, 1);
    }

    #[test]
    fn a_zero_rec_len_is_rejected_instead_of_looping_forever() {
        let mut block = dir_block(&[(2, ".", 2)]);
        block[4..6].copy_from_slice(&0u16.to_le_bytes());
        let mut out = empty_out();
        assert_eq!(parse_dir_entries(&block, &mut out), Err(Ext2Error::BadRecLen));
    }

    #[test]
    fn a_rec_len_past_the_block_is_rejected() {
        let mut block = dir_block(&[(2, ".", 2)]);
        block[4..6].copy_from_slice(&2000u16.to_le_bytes());
        let mut out = empty_out();
        assert_eq!(parse_dir_entries(&block, &mut out), Err(Ext2Error::BadRecLen));
    }

    #[test]
    fn a_name_longer_than_its_record_is_rejected() {
        // 唯一一项会吃掉整块，故先把 rec_len 改小（12 = 8 + 4 字节名字），
        // 再声称 name_len = 200：200 > 12 - 8 = 4，必须报错。
        let mut block = dir_block(&[(2, ".", 2)]);
        block[4..6].copy_from_slice(&12u16.to_le_bytes());
        block[6] = 200;
        let mut out = empty_out();
        assert_eq!(parse_dir_entries(&block, &mut out), Err(Ext2Error::BadNameLen));
    }

    #[test]
    fn deleted_entries_are_skipped_but_still_advance() {
        let block = dir_block(&[(0, "GONE", 1), (11, "KEEP", 1)]);
        let mut out = empty_out();
        let count = parse_dir_entries(&block, &mut out).expect("解析成功");
        assert_eq!(count, 1, "inode 为 0 的条目表示已删除");
        assert_eq!(&out[0].name[..4], b"KEEP");
    }
}

#[cfg(test)]
mod inode_tests {
    use super::{Ext2Error, Inode, parse_inode};
    use std::vec::Vec;

    /// 造一张 inode 表：第 n 个 inode（1 起算）的偏移是 size*(n-1)。
    fn inode_table(inode_size: usize, count: usize, entries: &[(usize, u16, u32, u32)]) -> Vec<u8> {
        let mut table = std::vec![0u8; inode_size * count];
        for (index, mode, size, first_block) in entries {
            let at = inode_size * (index - 1);
            table[at..at + 2].copy_from_slice(&mode.to_le_bytes());
            table[at + 4..at + 8].copy_from_slice(&size.to_le_bytes());
            table[at + 40..at + 44].copy_from_slice(&first_block.to_le_bytes());
        }
        table
    }

    #[test]
    fn inode_numbers_are_one_based() {
        // inode 2 的偏移应是 inode_size * 1，而不是 0。
        let table = inode_table(128, 4, &[(2, 0x81A4, 0x1234, 99)]);
        let inode: Inode = parse_inode(&table, 2, 128).expect("解析成功");
        assert_eq!(inode.mode, 0x81A4);
        assert_eq!(inode.size, 0x1234);
        assert_eq!(inode.blocks[0], 99);
    }

    #[test]
    fn inode_zero_is_rejected() {
        let table = inode_table(128, 4, &[]);
        assert_eq!(parse_inode(&table, 0, 128), Err(Ext2Error::BadInodeNumber));
    }

    #[test]
    fn an_inode_past_the_table_is_rejected() {
        let table = inode_table(128, 4, &[]);
        assert_eq!(parse_inode(&table, 5, 128), Err(Ext2Error::ShortImage));
    }

    #[test]
    fn a_larger_inode_size_scales_the_offset() {
        // inode_size = 256（revision 1 常见）：inode 2 的偏移是 256*1。
        let table = inode_table(256, 4, &[(2, 0x41ED, 4096, 7)]);
        let inode = parse_inode(&table, 2, 256).expect("解析成功");
        assert_eq!(inode.mode, 0x41ED);
        assert_eq!(inode.size, 4096);
        assert_eq!(inode.blocks[0], 7);
    }

    #[test]
    fn the_indirect_block_slots_are_readable() {
        // i_block[12] 是一级间接、[13] 二级、[14] 三级。
        let mut table = inode_table(128, 2, &[(1, 0x81A4, 0, 0)]);
        table[40 + 12 * 4..40 + 13 * 4].copy_from_slice(&111u32.to_le_bytes());
        table[40 + 13 * 4..40 + 14 * 4].copy_from_slice(&222u32.to_le_bytes());
        table[40 + 14 * 4..40 + 15 * 4].copy_from_slice(&333u32.to_le_bytes());
        let inode = parse_inode(&table, 1, 128).expect("解析成功");
        assert_eq!(inode.blocks[12], 111, "一级间接");
        assert_eq!(inode.blocks[13], 222, "二级间接");
        assert_eq!(inode.blocks[14], 333, "三级间接");
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::{Ext2Error, find_in_dir};
    use std::vec::Vec;

    fn dir_block(items: &[(u32, &str, u8)]) -> Vec<u8> {
        let mut block = std::vec![0u8; 1024];
        let mut at = 0usize;
        for (index, (inode, name, kind)) in items.iter().enumerate() {
            let need = 8 + name.len();
            let rec_len = if index + 1 == items.len() { 1024 - at } else { (need + 3) & !3 };
            block[at..at + 4].copy_from_slice(&inode.to_le_bytes());
            block[at + 4..at + 6].copy_from_slice(&(rec_len as u16).to_le_bytes());
            block[at + 6] = name.len() as u8;
            block[at + 7] = *kind;
            block[at + 8..at + 8 + name.len()].copy_from_slice(name.as_bytes());
            at += rec_len;
        }
        block
    }

    #[test]
    fn an_exact_name_is_found() {
        let block = dir_block(&[(2, ".", 2), (2, "..", 2), (11, "HELLO.TXT", 1)]);
        let found = find_in_dir(&block, b"HELLO.TXT").expect("查找成功");
        assert_eq!(found.map(|entry| entry.inode), Some(11));
    }

    #[test]
    fn the_search_is_case_sensitive() {
        let block = dir_block(&[(2, ".", 2), (11, "HELLO.TXT", 1)]);
        assert_eq!(find_in_dir(&block, b"hello.txt").expect("查找成功").map(|e| e.inode), None);
    }

    #[test]
    fn a_prefix_does_not_match_a_longer_name() {
        // "HELLO" 不应匹配 "HELLO.TXT"。
        let block = dir_block(&[(2, ".", 2), (11, "HELLO.TXT", 1)]);
        assert_eq!(find_in_dir(&block, b"HELLO").expect("查找成功").map(|e| e.inode), None);
    }

    #[test]
    fn a_missing_name_yields_none() {
        let block = dir_block(&[(2, ".", 2), (2, "..", 2)]);
        assert_eq!(find_in_dir(&block, b"NOPE").expect("查找成功").map(|e| e.inode), None);
    }

    #[test]
    fn a_malformed_directory_is_reported() {
        let mut block = dir_block(&[(2, ".", 2)]);
        block[4..6].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(find_in_dir(&block, b"."), Err(Ext2Error::BadRecLen));
    }
}
