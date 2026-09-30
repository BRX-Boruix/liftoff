//! FAT 只读（L4）—— BPB（BIOS Parameter Block）解析。
//!
//! 字段偏移按 **FAT 规范**书写。**如实说明**：本次会话**未**取到规范原文逐字段复核
//! （web 检索不可用），故属“按规范知识书写”，已记入台账待复核项。
//!
//! 判型算法：`root_entries == 0` → FAT32；否则算数据区簇数
//! （`data = total − (reserved + num_fats × fat_size + root_dir_sectors)`，
//! `clusters = data / sectors_per_cluster`）→ ≤ 4084 为 FAT12、≤ 65524 为 FAT16、否则 FAT32。

/// 引导扇区签名偏移。
pub const FAT_SIGNATURE_OFFSET: usize = 510;
/// 引导扇区长度。
pub const FAT_SECTOR_SIZE: usize = 512;
const FAT12_MAX_CLUSTERS: u32 = 4084;
const FAT16_MAX_CLUSTERS: u32 = 65524;

/// FAT 变体。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FatKind {
    /// FAT12。
    Fat12,
    /// FAT16。
    Fat16,
    /// FAT32。
    Fat32,
}

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FatError {
    /// 扇区不足 512 字节。
    ShortSector,
    /// 缺 `0x55AA` 签名。
    NotFat,
    /// 扇区大小为 0 或非 2 的幂。
    BadSectorSize,
    /// 每簇扇区数为 0 或非 2 的幂。
    BadClusterSize,
    /// FAT 数量为 0。
    BadFatCount,
    /// 判型算术溢出或布局自相矛盾。
    ArithmeticOverflow,
}

/// BPB 的关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bpb {
    /// 每扇区字节数。
    pub bytes_per_sector: u16,
    /// 每簇扇区数。
    pub sectors_per_cluster: u8,
    /// 保留扇区数。
    pub reserved_sectors: u16,
    /// FAT 表数量。
    pub num_fats: u8,
    /// 根目录项数（FAT32 为 0）。
    pub root_entries: u16,
    /// 总扇区数。
    pub total_sectors: u32,
    /// 每个 FAT 的扇区数。
    pub fat_size: u32,
    /// 根目录起始簇（FAT32）。
    pub root_cluster: u32,
    /// 判定的变体。
    pub kind: FatKind,
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

/// 解析引导扇区的 BPB。
pub fn parse_bpb(sector: &[u8]) -> Result<Bpb, FatError> {
    if sector.len() < FAT_SECTOR_SIZE {
        return Err(FatError::ShortSector);
    }
    if sector[FAT_SIGNATURE_OFFSET] != 0x55 || sector[FAT_SIGNATURE_OFFSET + 1] != 0xAA {
        return Err(FatError::NotFat);
    }
    let bytes_per_sector = read_u16(sector, 11).ok_or(FatError::ShortSector)?;
    if bytes_per_sector == 0 || !bytes_per_sector.is_power_of_two() {
        return Err(FatError::BadSectorSize);
    }
    let sectors_per_cluster = *sector.get(13).ok_or(FatError::ShortSector)?;
    if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
        return Err(FatError::BadClusterSize);
    }
    let reserved_sectors = read_u16(sector, 14).ok_or(FatError::ShortSector)?;
    let num_fats = *sector.get(16).ok_or(FatError::ShortSector)?;
    if num_fats == 0 {
        return Err(FatError::BadFatCount);
    }
    let root_entries = read_u16(sector, 17).ok_or(FatError::ShortSector)?;
    let total_16 = read_u16(sector, 19).ok_or(FatError::ShortSector)? as u32;
    let total_32 = read_u32(sector, 32).ok_or(FatError::ShortSector)?;
    let total_sectors = if total_16 != 0 { total_16 } else { total_32 };
    let fat_16 = read_u16(sector, 22).ok_or(FatError::ShortSector)? as u32;
    let fat_32 = read_u32(sector, 36).ok_or(FatError::ShortSector)?;
    let root_cluster = read_u32(sector, 44).ok_or(FatError::ShortSector)?;

    let (fat_size, kind) = if root_entries == 0 {
        (fat_32, FatKind::Fat32)
    } else {
        let fat_size = if fat_16 != 0 { fat_16 } else { fat_32 };
        let root_dir_bytes = (root_entries as u32)
            .checked_mul(32)
            .ok_or(FatError::ArithmeticOverflow)?;
        let root_dir_sectors = root_dir_bytes
            .checked_add(bytes_per_sector as u32 - 1)
            .ok_or(FatError::ArithmeticOverflow)?
            / bytes_per_sector as u32;
        let fats = (num_fats as u32)
            .checked_mul(fat_size)
            .ok_or(FatError::ArithmeticOverflow)?;
        let overhead = (reserved_sectors as u32)
            .checked_add(fats)
            .and_then(|value| value.checked_add(root_dir_sectors))
            .ok_or(FatError::ArithmeticOverflow)?;
        if total_sectors < overhead {
            return Err(FatError::ArithmeticOverflow);
        }
        let clusters = (total_sectors - overhead) / sectors_per_cluster as u32;
        let kind = if clusters <= FAT12_MAX_CLUSTERS {
            FatKind::Fat12
        } else if clusters <= FAT16_MAX_CLUSTERS {
            FatKind::Fat16
        } else {
            FatKind::Fat32
        };
        (fat_size, kind)
    };

    Ok(Bpb {
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        root_entries,
        total_sectors,
        fat_size,
        root_cluster,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::{FAT_SIGNATURE_OFFSET, FatError, FatKind, parse_bpb};
    use std::vec::Vec;

    /// 造一个 BPB 扇区（512 字节）。
    fn sector(
        bytes_per_sector: u16,
        sectors_per_cluster: u8,
        reserved: u16,
        num_fats: u8,
        root_entries: u16,
        total_sectors: u32,
        fat_size_16: u16,
        fat_size_32: u32,
        root_cluster: u32,
    ) -> Vec<u8> {
        let mut raw = std::vec![0u8; 512];
        raw[0] = 0xEB;
        raw[11..13].copy_from_slice(&bytes_per_sector.to_le_bytes());
        raw[13] = sectors_per_cluster;
        raw[14..16].copy_from_slice(&reserved.to_le_bytes());
        raw[16] = num_fats;
        raw[17..19].copy_from_slice(&root_entries.to_le_bytes());
        raw[19..21].copy_from_slice(&(total_sectors.min(0xFFFF) as u16).to_le_bytes());
        raw[22..24].copy_from_slice(&fat_size_16.to_le_bytes());
        raw[32..36].copy_from_slice(&total_sectors.to_le_bytes());
        raw[36..40].copy_from_slice(&fat_size_32.to_le_bytes());
        raw[44..48].copy_from_slice(&root_cluster.to_le_bytes());
        raw[510] = 0x55;
        raw[511] = 0xAA;
        raw
    }

    #[test]
    fn a_fat16_bpb_yields_its_fields() {
        // 2 扇区/簇、总 65536 扇区、FAT 大小 256 → 数据区约 64768 扇区 → 32384 簇（FAT16 区间）。
        let raw = sector(512, 2, 1, 2, 512, 65536, 256, 0, 0);
        let bpb = parse_bpb(&raw).expect("BPB 有效");
        assert_eq!(bpb.bytes_per_sector, 512);
        assert_eq!(bpb.sectors_per_cluster, 2);
        assert_eq!(bpb.reserved_sectors, 1);
        assert_eq!(bpb.num_fats, 2);
        assert_eq!(bpb.root_entries, 512);
        assert_eq!(bpb.fat_size, 256, "FAT16 的 FAT 大小在偏移 22");
        assert_eq!(bpb.kind, FatKind::Fat16);
    }

    #[test]
    fn a_fat32_bpb_reads_the_32_bit_fat_size() {
        // root_entries == 0 → FAT32；FAT 大小在偏移 36，根目录簇在偏移 44。
        let raw = sector(512, 8, 32, 2, 0, 0x0020_0000, 0, 4096, 2);
        let bpb = parse_bpb(&raw).expect("BPB 有效");
        assert_eq!(bpb.root_entries, 0);
        assert_eq!(bpb.fat_size, 4096, "FAT32 的 FAT 大小在偏移 36");
        assert_eq!(bpb.root_cluster, 2);
        assert_eq!(bpb.kind, FatKind::Fat32);
    }

    #[test]
    fn a_small_volume_is_detected_as_fat12() {
        // 数据区很小 → 簇数 < 4085 → FAT12。
        let raw = sector(512, 1, 1, 2, 16, 2048, 6, 0, 0);
        let bpb = parse_bpb(&raw).expect("BPB 有效");
        assert_eq!(bpb.kind, FatKind::Fat12);
    }

    #[test]
    fn a_bad_signature_is_rejected() {
        let mut raw = sector(512, 2, 1, 2, 512, 65536, 256, 0, 0);
        raw[FAT_SIGNATURE_OFFSET] = 0;
        assert_eq!(parse_bpb(&raw), Err(FatError::NotFat));
    }

    #[test]
    fn a_non_power_of_two_sector_size_is_rejected() {
        let raw = sector(1000, 2, 1, 2, 512, 65536, 256, 0, 0);
        assert_eq!(parse_bpb(&raw), Err(FatError::BadSectorSize));
    }

    #[test]
    fn a_zero_fat_count_is_rejected() {
        let raw = sector(512, 2, 1, 0, 512, 65536, 256, 0, 0);
        assert_eq!(parse_bpb(&raw), Err(FatError::BadFatCount));
    }

    #[test]
    fn a_zero_sectors_per_cluster_is_rejected() {
        let raw = sector(512, 0, 1, 2, 512, 65536, 256, 0, 0);
        assert_eq!(parse_bpb(&raw), Err(FatError::BadClusterSize));
    }
}