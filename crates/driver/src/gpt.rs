//! GPT 解析（L4）。
//!
//! 决策：**校验头与项数组的 CRC32** —— 读错分区 = 读错数据，不信任未校验的扇区。
//!
//! 头布局（位于 LBA 1，偏移 = 扇区大小）：签名 `EFI PART` @0、`header_size` @12（u32）、
//! 头 CRC32 @16、`current_lba` @24、`backup_lba` @32、`first_usable` @40、`last_usable` @48、
//! 磁盘 GUID @56、`entries_lba` @72（u64）、`entry_count` @80（u32）、`entry_size` @84（u32）、
//! 项数组 CRC32 @88。全部**小端**。

use crate::crc32::crc32;
use crate::partition::Partition;

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
    /// 项数组 CRC32 不匹配（表被破坏）。
    EntriesCrcMismatch,
    /// 项数组越出映像范围。
    EntriesOutOfBounds,
    /// 分区项区间倒置（`last < first`，减法会下溢）。
    BadEntryRange,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
}

impl core::fmt::Display for GptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ShortImage => f.write_str("映像不足，连 GPT 头都放不下"),
            Self::NotGpt => f.write_str("签名不是 EFI PART"),
            Self::BadHeaderSize => f.write_str("header_size 不合理"),
            Self::HeaderCrcMismatch => f.write_str("GPT 头 CRC32 不匹配（表被破坏）"),
            Self::BadEntrySize => f.write_str("entry_size 不合理"),
            Self::EntriesCrcMismatch => f.write_str("分区项数组 CRC32 不匹配（表被破坏）"),
            Self::EntriesOutOfBounds => f.write_str("分区项数组越出映像范围"),
            Self::BadEntryRange => f.write_str("分区项区间倒置（last 小于 first）"),
            Self::BufferTooSmall => f.write_str("调用方给的输出缓冲太小"),
        }
    }
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

/// 解析 GPT 分区项数组（含项数组 CRC32 校验），返回分区数。
///
/// 全零类型 GUID 的项表示“未使用”，跳过；`last < first` 视为表损坏，报 `BadEntryRange`
/// （绝不回绕成巨大扇区数）。所有偏移与长度都用 checked 运算，防溢出。
pub fn parse_gpt_entries(
    image: &[u8],
    sector_size: u64,
    header: &GptHeader,
    out: &mut [Partition],
) -> Result<usize, GptError> {
    let start = usize::try_from(
        header
            .entries_lba
            .checked_mul(sector_size)
            .ok_or(GptError::EntriesOutOfBounds)?,
    )
    .map_err(|_| GptError::EntriesOutOfBounds)?;
    let total = (header.entry_count as usize)
        .checked_mul(header.entry_size as usize)
        .ok_or(GptError::EntriesOutOfBounds)?;
    let end = start.checked_add(total).ok_or(GptError::EntriesOutOfBounds)?;
    let raw = image.get(start..end).ok_or(GptError::EntriesOutOfBounds)?;
    if crc32(raw) != header.entries_crc {
        return Err(GptError::EntriesCrcMismatch);
    }
    let entry_size = header.entry_size as usize;
    let mut count = 0;
    for index in 0..header.entry_count as usize {
        let at = index.checked_mul(entry_size).ok_or(GptError::EntriesOutOfBounds)?;
        let item = raw.get(at..at + entry_size).ok_or(GptError::EntriesOutOfBounds)?;
        if item.get(..16).map_or(true, |guid| guid.iter().all(|byte| *byte == 0)) {
            continue;
        }
        let first = read_u64(item, 32).ok_or(GptError::EntriesOutOfBounds)?;
        let last = read_u64(item, 40).ok_or(GptError::EntriesOutOfBounds)?;
        let sector_count = last
            .checked_sub(first)
            .ok_or(GptError::BadEntryRange)?
            .checked_add(1)
            .ok_or(GptError::BadEntryRange)?;
        if count == out.len() {
            return Err(GptError::BufferTooSmall);
        }
        out[count] = Partition { start_lba: first, sector_count };
        count += 1;
    }
    Ok(count)
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

#[cfg(test)]
mod entry_tests {
    use super::{GPT_ENTRY_SIZE, GptError, GptHeader, Partition, parse_gpt_entries};
    use crate::crc32::crc32;
    use std::vec::Vec;

    const SECTOR: usize = 512;
    const ENTRY_COUNT: u32 = 4;

    /// 造一项：类型 GUID 首字节非零表示“已使用”。
    fn entry(used: bool, first: u64, last: u64) -> [u8; 128] {
        let mut item = [0u8; 128];
        if used {
            item[0] = 0x0F;
        }
        item[32..40].copy_from_slice(&first.to_le_bytes());
        item[40..48].copy_from_slice(&last.to_le_bytes());
        item
    }

    /// 造一个 3 扇区映像：头在 LBA 1，项数组在 LBA 2。
    fn image(entries: &[[u8; 128]]) -> Vec<u8> {
        let mut raw = std::vec![0u8; 3 * SECTOR];
        for (index, item) in entries.iter().enumerate() {
            let at = 2 * SECTOR + index * 128;
            raw[at..at + 128].copy_from_slice(item);
        }
        let crc = crc32(&raw[2 * SECTOR..2 * SECTOR + ENTRY_COUNT as usize * 128]);
        // 头：签名 + header_size + 项数组信息
        raw[SECTOR..SECTOR + 8].copy_from_slice(b"EFI PART");
        raw[SECTOR + 12..SECTOR + 16].copy_from_slice(&92u32.to_le_bytes());
        raw[SECTOR + 72..SECTOR + 80].copy_from_slice(&2u64.to_le_bytes());
        raw[SECTOR + 80..SECTOR + 84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        raw[SECTOR + 84..SECTOR + 88].copy_from_slice(&(GPT_ENTRY_SIZE).to_le_bytes());
        raw[SECTOR + 88..SECTOR + 92].copy_from_slice(&crc.to_le_bytes());
        raw
    }

    fn header(entries_crc: u32) -> GptHeader {
        GptHeader { entries_lba: 2, entry_count: ENTRY_COUNT, entry_size: GPT_ENTRY_SIZE, entries_crc }
    }

    #[test]
    fn used_entries_are_returned_with_their_span() {
        let items = [
            entry(true, 2048, 4095),
            entry(true, 8192, 8192),
            entry(false, 0, 0),
            entry(false, 0, 0),
        ];
        let raw = image(&items);
        let crc = crc32(&raw[2 * SECTOR..2 * SECTOR + ENTRY_COUNT as usize * 128]);
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        let count = parse_gpt_entries(&raw, SECTOR as u64, &header(crc), &mut out).expect("解析成功");
        assert_eq!(count, 2, "全零 GUID 的项应被跳过");
        assert_eq!(out[0].start_lba, 2048);
        assert_eq!(out[0].sector_count, 2048, "4095-2048+1");
        assert_eq!(out[1].start_lba, 8192);
        assert_eq!(out[1].sector_count, 1);
    }

    #[test]
    fn a_bad_entries_crc_is_rejected() {
        let items = [entry(true, 2048, 4095), entry(false, 0, 0), entry(false, 0, 0), entry(false, 0, 0)];
        let raw = image(&items);
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        assert_eq!(
            parse_gpt_entries(&raw, SECTOR as u64, &header(0xDEAD_BEEF), &mut out),
            Err(GptError::EntriesCrcMismatch)
        );
    }

    #[test]
    fn entries_past_the_end_of_the_image_are_rejected() {
        let raw = std::vec![0u8; 2 * SECTOR];
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        let head = GptHeader { entries_lba: 2, entry_count: ENTRY_COUNT, entry_size: GPT_ENTRY_SIZE, entries_crc: 0 };
        assert_eq!(
            parse_gpt_entries(&raw, SECTOR as u64, &head, &mut out),
            Err(GptError::EntriesOutOfBounds)
        );
    }

    #[test]
    fn an_inverted_span_is_rejected() {
        let items = [entry(true, 4095, 2048), entry(false, 0, 0), entry(false, 0, 0), entry(false, 0, 0)];
        let raw = image(&items);
        let crc = crc32(&raw[2 * SECTOR..2 * SECTOR + ENTRY_COUNT as usize * 128]);
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        assert_eq!(
            parse_gpt_entries(&raw, SECTOR as u64, &header(crc), &mut out),
            Err(GptError::BadEntryRange)
        );
    }
}
