//! 分区表判定与解析的组合入口（L4）。
//!
//! 判定顺序（**重要**）：先看第 0 扇区是否为**保护性 MBR** → 是则**只走 GPT**
//! （绝不把保护性 MBR 当普通 MBR 返回 ✗，也绝不在 GPT 损坏时回退 ✗ ——
//! 回退会把整块盘当成一个分区，后果严重）；否则若为普通 MBR → 走 MBR；
//! 否则 `None`（无分区表的盘是**合法**情形，不是错误）。

use crate::gpt::{GptError, is_protective_mbr, parse_gpt_entries, parse_gpt_header};
use crate::partition::{Partition, PartitionError, parse_mbr};

/// 分区表种类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableKind {
    /// MBR 分区表。
    Mbr,
    /// GPT 分区表。
    Gpt,
}

/// 组合入口的错误：**不做有损扁平化**，保留是哪一层出的错。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableError {
    /// MBR 层出错。
    Partition(PartitionError),
    /// GPT 层出错。
    Gpt(GptError),
}

impl From<PartitionError> for TableError {
    fn from(error: PartitionError) -> Self {
        Self::Partition(error)
    }
}

impl From<GptError> for TableError {
    fn from(error: GptError) -> Self {
        Self::Gpt(error)
    }
}

/// 判定并解析分区表；`Ok(None)` 表示没有分区表。
pub fn parse_partition_table(
    sector0: &[u8],
    image: &[u8],
    sector_size: u64,
    out: &mut [Partition],
) -> Result<Option<(TableKind, usize)>, TableError> {
    if is_protective_mbr(sector0) {
        let header = parse_gpt_header(image, sector_size)?;
        let count = parse_gpt_entries(image, sector_size, &header, out)?;
        return Ok(Some((TableKind::Gpt, count)));
    }
    match parse_mbr(sector0, out) {
        Ok(count) => Ok(Some((TableKind::Mbr, count))),
        Err(PartitionError::NotAPartitionTable) => Ok(None),
        Err(other) => Err(TableError::Partition(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::{TableError, TableKind, parse_partition_table};
    use crate::crc32::crc32;
    use crate::gpt::GPT_ENTRY_SIZE;
    use crate::partition::Partition;
    use std::vec::Vec;

    const SECTOR: usize = 512;
    const ENTRY_COUNT: u32 = 4;

    /// 保护性 MBR 扇区。
    fn protective_mbr() -> Vec<u8> {
        let mut sector = std::vec![0u8; SECTOR];
        sector[446 + 4] = 0xEE;
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector
    }

    /// 普通 MBR：一个 0x83 分区（LBA 2048、4096 扇区）。
    fn plain_mbr() -> Vec<u8> {
        let mut sector = std::vec![0u8; SECTOR];
        sector[446 + 4] = 0x83;
        sector[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
        sector[446 + 12..446 + 16].copy_from_slice(&4096u32.to_le_bytes());
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector
    }

    /// 3 扇区映像：GPT 头在 LBA 1，一个有效分区项在 LBA 2。
    fn gpt_image(break_crc: bool) -> Vec<u8> {
        let mut raw = std::vec![0u8; 3 * SECTOR];
        raw[2 * SECTOR] = 0x0F;
        raw[2 * SECTOR + 32..2 * SECTOR + 40].copy_from_slice(&2048u64.to_le_bytes());
        raw[2 * SECTOR + 40..2 * SECTOR + 48].copy_from_slice(&4095u64.to_le_bytes());
        let entries_crc = crc32(&raw[2 * SECTOR..2 * SECTOR + ENTRY_COUNT as usize * 128]);
        raw[SECTOR..SECTOR + 8].copy_from_slice(b"EFI PART");
        raw[SECTOR + 12..SECTOR + 16].copy_from_slice(&92u32.to_le_bytes());
        raw[SECTOR + 72..SECTOR + 80].copy_from_slice(&2u64.to_le_bytes());
        raw[SECTOR + 80..SECTOR + 84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        raw[SECTOR + 84..SECTOR + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        raw[SECTOR + 88..SECTOR + 92].copy_from_slice(&entries_crc.to_le_bytes());
        let header_crc = crc32(&raw[SECTOR..SECTOR + 92]);
        let header_crc = if break_crc { header_crc ^ 0xFF } else { header_crc };
        raw[SECTOR + 16..SECTOR + 20].copy_from_slice(&header_crc.to_le_bytes());
        raw
    }

    fn empty_out() -> [Partition; 4] {
        [Partition { start_lba: 0, sector_count: 0 }; 4]
    }

    #[test]
    fn a_protective_mbr_with_a_valid_gpt_yields_gpt_partitions() {
        let raw = gpt_image(false);
        let mut out = empty_out();
        let parsed = parse_partition_table(&protective_mbr(), &raw, SECTOR as u64, &mut out).expect("解析成功");
        assert_eq!(parsed, Some((TableKind::Gpt, 1)));
        assert_eq!(out[0].start_lba, 2048);
        assert_eq!(out[0].sector_count, 2048);
    }

    #[test]
    fn a_plain_mbr_yields_mbr_partitions() {
        let raw = std::vec![0u8; 2 * SECTOR];
        let mut out = empty_out();
        let parsed = parse_partition_table(&plain_mbr(), &raw, SECTOR as u64, &mut out).expect("解析成功");
        assert_eq!(parsed, Some((TableKind::Mbr, 1)), "普通 MBR 不得被当成 GPT");
        assert_eq!(out[0].start_lba, 2048);
    }

    #[test]
    fn neither_table_yields_none() {
        let sector = std::vec![0u8; SECTOR];
        let raw = std::vec![0u8; 2 * SECTOR];
        let mut out = empty_out();
        assert_eq!(
            parse_partition_table(&sector, &raw, SECTOR as u64, &mut out).expect("不报错"),
            None
        );
    }

    #[test]
    fn a_broken_gpt_is_reported_and_never_falls_back_to_mbr() {
        let raw = gpt_image(true);
        let mut out = empty_out();
        let parsed = parse_partition_table(&protective_mbr(), &raw, SECTOR as u64, &mut out);
        assert!(
            matches!(parsed, Err(TableError::Gpt(_))),
            "GPT 损坏必须报错，绝不能回退成 MBR：{parsed:?}"
        );
    }
}