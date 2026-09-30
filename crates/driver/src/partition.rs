//! 分区表解析：MBR（L4）。
//!
//! 边界：**纯字节解析**（只依赖 `core`）；介质读取由调用方提供。
//!
//! MBR 布局（512 字节扇区，已对照通用规范）：分区项表在偏移 446，共 4 项、每项 16 字节
//! （+0 引导标志、+4 类型、+8 起始 LBA（小端 u32）、+12 扇区数（小端 u32））；
//! 签名 `0x55AA` 在偏移 510。

/// 一个分区（LBA 与扇区数）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Partition {
    /// 起始逻辑块地址（LBA）。
    pub start_lba: u64,
    /// 扇区数。
    pub sector_count: u64,
}

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PartitionError {
    /// 扇区不足 512 字节。
    ShortSector,
    /// 缺 `0x55AA` 签名。
    NotAPartitionTable,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
}

const ENTRY_TABLE_OFFSET: usize = 446;
const ENTRY_SIZE: usize = 16;
const ENTRY_COUNT: usize = 4;
const SIGNATURE_OFFSET: usize = 510;
const SECTOR_SIZE: usize = 512;

fn read_u32(sector: &[u8], at: usize) -> Option<u32> {
    let bytes = sector.get(at..at + 4)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    Some(u32::from_le_bytes(buf))
}

/// 解析 MBR，写入 `out`，返回分区数（类型为 0 或扇区数为 0 的条目被跳过）。
pub fn parse_mbr(sector: &[u8], out: &mut [Partition]) -> Result<usize, PartitionError> {
    // 先判长度再读，绝不越界。
    if sector.len() < SECTOR_SIZE {
        return Err(PartitionError::ShortSector);
    }
    if sector[SIGNATURE_OFFSET] != 0x55 || sector[SIGNATURE_OFFSET + 1] != 0xAA {
        return Err(PartitionError::NotAPartitionTable);
    }
    let mut count = 0;
    for index in 0..ENTRY_COUNT {
        let at = ENTRY_TABLE_OFFSET + index * ENTRY_SIZE;
        let kind = sector[at + 4];
        let start_lba = read_u32(sector, at + 8).ok_or(PartitionError::ShortSector)? as u64;
        let sector_count = read_u32(sector, at + 12).ok_or(PartitionError::ShortSector)? as u64;
        if kind == 0 || sector_count == 0 {
            continue;
        }
        if count == out.len() {
            return Err(PartitionError::BufferTooSmall);
        }
        out[count] = Partition { start_lba, sector_count };
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{Partition, PartitionError, parse_mbr};
    use std::vec::Vec;

    /// 造一个 512 字节扇区，带 0x55AA 签名与给定的四个分区项。
    fn mbr(entries: [(u8, u32, u32); 4]) -> Vec<u8> {
        let mut sector = std::vec![0u8; 512];
        for (index, (kind, lba, count)) in entries.iter().enumerate() {
            let at = 446 + index * 16;
            sector[at] = 0x00;
            sector[at + 4] = *kind;
            sector[at + 8..at + 12].copy_from_slice(&lba.to_le_bytes());
            sector[at + 12..at + 16].copy_from_slice(&count.to_le_bytes());
        }
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector
    }

    #[test]
    fn a_valid_mbr_yields_its_partitions_in_order() {
        let sector = mbr([(0x83, 2048, 4096), (0xEF, 6144, 8192), (0, 0, 0), (0, 0, 0)]);
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        let count = parse_mbr(&sector, &mut out).expect("解析成功");
        assert_eq!(count, 2, "type 为 0 的条目应被跳过");
        assert_eq!(out[0].start_lba, 2048);
        assert_eq!(out[0].sector_count, 4096);
        assert_eq!(out[1].start_lba, 6144);
    }

    #[test]
    fn a_missing_signature_is_rejected() {
        let mut sector = mbr([(0x83, 2048, 4096), (0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        sector[510] = 0;
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        assert_eq!(parse_mbr(&sector, &mut out), Err(PartitionError::NotAPartitionTable));
    }

    #[test]
    fn a_short_sector_is_rejected() {
        let sector = std::vec![0u8; 256];
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        assert_eq!(parse_mbr(&sector, &mut out), Err(PartitionError::ShortSector));
    }

    #[test]
    fn a_too_small_output_buffer_is_reported() {
        let sector = mbr([(0x83, 2048, 4096), (0xEF, 6144, 8192), (0, 0, 0), (0, 0, 0)]);
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 1];
        assert_eq!(parse_mbr(&sector, &mut out), Err(PartitionError::BufferTooSmall));
    }

    #[test]
    fn a_zero_sector_count_entry_is_skipped() {
        let sector = mbr([(0x83, 2048, 0), (0xEF, 6144, 8192), (0, 0, 0), (0, 0, 0)]);
        let mut out = [Partition { start_lba: 0, sector_count: 0 }; 4];
        let count = parse_mbr(&sector, &mut out).expect("解析成功");
        assert_eq!(count, 1);
        assert_eq!(out[0].start_lba, 6144);
    }
}