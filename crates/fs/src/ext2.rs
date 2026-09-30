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