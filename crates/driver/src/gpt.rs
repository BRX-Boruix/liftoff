//! GPT 解析（L4）。
//!
//! 决策：**校验头与项数组的 CRC32** —— 读错分区 = 读错数据，不信任未校验的扇区。
//!
//! 头布局（位于 LBA 1，偏移 = 扇区大小）：签名 `EFI PART` @0、`header_size` @12（u32）、
//! 头 CRC32 @16、`current_lba` @24、`backup_lba` @32、`first_usable` @40、`last_usable` @48、
//! 磁盘 GUID @56、`entries_lba` @72（u64）、`entry_count` @80（u32）、`entry_size` @84（u32）、
//! 项数组 CRC32 @88。全部**小端**。

use crate::crc32::crc32;

/// GPT 签名。
pub const GPT_SIGNATURE: [u8; 8] = *b"EFI PART";
/// GPT 头固定部分的大小。
pub const GPT_HEADER_SIZE: usize = 92;
/// 分区项大小的规范值（项大小必须是它的整数倍）。
pub const GPT_ENTRY_SIZE: u32 = 128;

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GptError {
    /// 映像不足（连头都放不下）。
    ShortImage,
    /// 签名不是 `EFI PART`。
    NotGpt,
    /// `header_size` 不合理。
    BadHeaderSize,
    /// 头 CRC32 不匹配（表被破坏）。
    HeaderCrcMismatch,
    /// `entry_size` 不合理。
    BadEntrySize,
}

/// GPT 头的关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GptHeader {
    /// 分区项数组所在 LBA。
    pub entries_lba: u64,
    /// 分区项数量。
    pub entry_count: u32,
    /// 每项大小。
    pub entry_size: u32,
    /// 项数组的 CRC32。
    pub entries_crc: u32,
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let raw = bytes.get(at..at.checked_add(4)?)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(raw);
    Some(u32::from_le_bytes(buf))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let raw = bytes.get(at..at.checked_add(8)?)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(raw);
    Some(u64::from_le_bytes(buf))
}

/// 判断第 0 扇区是否为**保护性 MBR**（类型 `0xEE` + `0x55AA` 签名）。
pub fn is_protective_mbr(sector: &[u8]) -> bool {
    if sector.len() < 512 {
        return false;
    }
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return false;
    }
    sector[446 + 4] == 0xEE
}

/// 解析 GPT 头（含头 CRC32 校验）。
pub fn parse_gpt_header(image: &[u8], sector_size: u64) -> Result<GptHeader, GptError> {
    let base = usize::try_from(sector_size).map_err(|_| GptError::ShortImage)?;
    let end = base.checked_add(GPT_HEADER_SIZE).ok_or(GptError::ShortImage)?;
    let head = image.get(base..end).ok_or(GptError::ShortImage)?;
    if head.get(0..8) != Some(&GPT_SIGNATURE[..]) {
        return Err(GptError::NotGpt);
    }
    let header_size = read_u32(head, 12).ok_or(GptError::ShortImage)? as usize;
    if header_size < GPT_HEADER_SIZE || header_size > sector_size as usize {
        return Err(GptError::BadHeaderSize);
    }
    let stored = read_u32(head, 16).ok_or(GptError::ShortImage)?;
    // 校验时把 CRC 字段自身清零（规范要求，最容易写错的一步）。
    let mut copy = [0u8; 512];
    let slice = copy.get_mut(..header_size).ok_or(GptError::BadHeaderSize)?;
    slice.copy_from_slice(head.get(..header_size).ok_or(GptError::ShortImage)?);
    copy[16..20].copy_from_slice(&[0, 0, 0, 0]);
    if crc32(&copy[..header_size]) != stored {
        return Err(GptError::HeaderCrcMismatch);
    }
    let entry_size = read_u32(head, 84).ok_or(GptError::ShortImage)?;
    if entry_size < GPT_ENTRY_SIZE || entry_size % GPT_ENTRY_SIZE != 0 {
        return Err(GptError::BadEntrySize);
    }
    Ok(GptHeader {
        entries_lba: read_u64(head, 72).ok_or(GptError::ShortImage)?,
        entry_count: read_u32(head, 80).ok_or(GptError::ShortImage)?,
        entry_size,
        entries_crc: read_u32(head, 88).ok_or(GptError::ShortImage)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{GptError, GPT_HEADER_SIZE, GPT_SIGNATURE, is_protective_mbr, parse_gpt_header};
    use crate::crc32::crc32;
    use std::vec::Vec;

    const SECTOR: usize = 512;

    /// 造一个 GPT 头（92 字节有效，其余补零到 header_size）。
    fn header(entries_lba: u64, entry_count: u32, entry_size: u32, entries_crc: u32) -> Vec<u8> {
        let mut head = std::vec![0u8; GPT_HEADER_SIZE];
        head[0..8].copy_from_slice(&GPT_SIGNATURE);
        head[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        head[12..16].copy_from_slice(&(GPT_HEADER_SIZE as u32).to_le_bytes());
        // 头 CRC 先留 0，最后回填
        head[24..32].copy_from_slice(&1u64.to_le_bytes());
        head[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        head[80..84].copy_from_slice(&entry_count.to_le_bytes());
        head[84..88].copy_from_slice(&entry_size.to_le_bytes());
        head[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let crc = crc32(&head);
        head[16..20].copy_from_slice(&crc.to_le_bytes());
        head
    }

    fn image_with_header(head: &[u8]) -> Vec<u8> {
        let mut image = std::vec![0u8; 2 * SECTOR];
        image[SECTOR..SECTOR + head.len()].copy_from_slice(head);
        image
    }

    #[test]
    fn a_protective_mbr_is_recognised() {
        let mut sector = std::vec![0u8; SECTOR];
        sector[446 + 4] = 0xEE;
        sector[510] = 0x55;
        sector[511] = 0xAA;
        assert!(is_protective_mbr(&sector));
        sector[446 + 4] = 0x83;
        assert!(!is_protective_mbr(&sector), "普通 MBR 不是保护性 MBR");
    }

    #[test]
    fn a_valid_header_yields_its_fields() {
        let image = image_with_header(&header(2, 128, 128, 0x1234_5678));
        let head = parse_gpt_header(&image, SECTOR as u64).expect("头有效");
        assert_eq!(head.entries_lba, 2);
        assert_eq!(head.entry_count, 128);
        assert_eq!(head.entry_size, 128);
        assert_eq!(head.entries_crc, 0x1234_5678);
    }

    #[test]
    fn a_bad_signature_is_rejected() {
        let mut head = header(2, 128, 128, 0);
        head[0] = b'X';
        let image = image_with_header(&head);
        assert_eq!(parse_gpt_header(&image, SECTOR as u64), Err(GptError::NotGpt));
    }

    #[test]
    fn a_bad_header_crc_is_rejected() {
        let mut head = header(2, 128, 128, 0);
        head[16] ^= 0xFF;
        let image = image_with_header(&head);
        assert_eq!(parse_gpt_header(&image, SECTOR as u64), Err(GptError::HeaderCrcMismatch));
    }

    #[test]
    fn a_short_image_is_rejected() {
        let image = std::vec![0u8; SECTOR];
        assert_eq!(parse_gpt_header(&image, SECTOR as u64), Err(GptError::ShortImage));
    }

    #[test]
    fn an_absurd_entry_size_is_rejected() {
        let image = image_with_header(&header(2, 128, 3, 0));
        assert_eq!(parse_gpt_header(&image, SECTOR as u64), Err(GptError::BadEntrySize));
    }
}