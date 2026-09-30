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
    /// 表项越出 FAT 表范围。
    TableOutOfBounds,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
    /// 链上遇到坏簇。
    BadCluster,
    /// 链断裂（遇到空闲簇 0）。
    BrokenChain,
    /// 簇链回环（游标不前进）。
    ChainLoop,
    /// 簇号超出卷范围。
    ClusterOutOfRange,
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

/// 读 FAT 表里第 `index` 个表项。
///
/// FAT12 的项是 **12 位**且**跨字节边界**：项 n 的字节偏移是 `n + n / 2`；
/// **偶项**取低 12 位、**奇项**取高 12 位（右移 4）。这是 FAT12 最经典的陷阱。
/// FAT32 的高 4 位是**保留位**，必须屏蔽（否则会读出 0xFFFF_FFFF 这类非法值）。
/// 越界一律报错（先判范围再读，不越界读）。
pub fn fat_entry(fat: &[u8], index: u32, kind: FatKind) -> Result<u32, FatError> {
    match kind {
        FatKind::Fat12 => {
            let slot = index as usize;
            let offset = slot
                .checked_add(slot / 2)
                .ok_or(FatError::TableOutOfBounds)?;
            let raw = read_u16(fat, offset).ok_or(FatError::TableOutOfBounds)? as u32;
            Ok(if slot % 2 == 0 { raw & 0x0FFF } else { raw >> 4 })
        }
        FatKind::Fat16 => {
            let offset = (index as usize)
                .checked_mul(2)
                .ok_or(FatError::TableOutOfBounds)?;
            Ok(read_u16(fat, offset).ok_or(FatError::TableOutOfBounds)? as u32)
        }
        FatKind::Fat32 => {
            let offset = (index as usize)
                .checked_mul(4)
                .ok_or(FatError::TableOutOfBounds)?;
            Ok(read_u32(fat, offset).ok_or(FatError::TableOutOfBounds)? & 0x0FFF_FFFF)
        }
    }
}

/// 链尾判断（各变体的结束标记区间不同）。
pub fn is_end_of_chain(kind: FatKind, value: u32) -> bool {
    match kind {
        FatKind::Fat12 => (0x0FF8..=0x0FFF).contains(&value),
        FatKind::Fat16 => (0xFFF8..=0xFFFF).contains(&value),
        FatKind::Fat32 => (0x0FFF_FFF8..=0x0FFF_FFFF).contains(&value),
    }
}

/// 坏簇判断。**与链尾互斥**：坏簇是错误，不是链的结束。
pub fn is_bad_cluster(kind: FatKind, value: u32) -> bool {
    match kind {
        FatKind::Fat12 => value == 0x0FF7,
        FatKind::Fat16 => value == 0xFFF7,
        FatKind::Fat32 => value == 0x0FFF_FFF7,
    }
}

/// 一个 8.3 目录项（名字 11 字节，空格填充）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    /// 8.3 名（11 字节）。
    pub name: [u8; 11],
    /// 属性字节。
    pub attributes: u8,
    /// 起始簇（含 FAT32 的高 16 位）。
    pub first_cluster: u32,
    /// 文件大小（字节）。
    pub size: u32,
}

impl Entry {
    /// 占位值。
    pub const EMPTY: Self = Self { name: [0; 11], attributes: 0, first_cluster: 0, size: 0 };
}

/// 遍历一个目录扇区里的 32 字节目录项，返回条目数。
///
/// 首字节 `0x00` 表示**后续全空** → 终止遍历；`0xE5` 表示**已删除** → 跳过；
/// 属性 `0x0F` 是**长文件名（LFN）项** → **明确跳过**（本实现不支持 LFN，不假装支持）。
pub fn parse_directory(sector: &[u8], out: &mut [Entry]) -> Result<usize, FatError> {
    if sector.len() < 32 {
        return Err(FatError::ShortSector);
    }
    let mut count = 0;
    let mut at = 0usize;
    while at + 32 <= sector.len() {
        let item = sector.get(at..at + 32).ok_or(FatError::ShortSector)?;
        let first = item[0];
        if first == 0x00 {
            break;
        }
        if first == 0xE5 {
            at += 32;
            continue;
        }
        let attributes = item[11];
        if attributes == 0x0F {
            at += 32;
            continue;
        }
        if count == out.len() {
            return Err(FatError::BufferTooSmall);
        }
        let mut entry = Entry::EMPTY;
        entry.name.copy_from_slice(&item[..11]);
        entry.attributes = attributes;
        let low = u16::from_le_bytes([item[26], item[27]]) as u32;
        let high = u16::from_le_bytes([item[20], item[21]]) as u32;
        entry.first_cluster = (high << 16) | low;
        entry.size = u32::from_le_bytes([item[28], item[29], item[30], item[31]]);
        out[count] = entry;
        count += 1;
        at += 32;
    }
    Ok(count)
}

/// 按簇链读取文件内容，返回写入的字节数。
///
/// 命门是**游标必须前进**：链尾正常结束；坏簇 → `BadCluster`；空闲簇 0 → `BrokenChain`；
/// 簇号超出卷 → `ClusterOutOfRange`；**回环 → `ChainLoop`**。
///
/// 回环判定用**步数上限 = 总簇数**（一条链最多访问 `total_clusters` 个不同簇），
/// **不假设簇号递增** —— 簇号在链中并无顺序保证。
pub fn read_chain<F>(
    fat: &[u8],
    start_cluster: u32,
    kind: FatKind,
    total_clusters: u32,
    cluster_size: usize,
    out: &mut [u8],
    mut read_cluster: F,
) -> Result<usize, FatError>
where
    F: FnMut(u32, &mut [u8]) -> Result<(), FatError>,
{
    if cluster_size == 0 {
        return Err(FatError::BadClusterSize);
    }
    let mut written = 0usize;
    let mut cluster = start_cluster;
    let mut steps = 0u32;
    loop {
        if cluster == 0 || cluster >= total_clusters {
            return Err(if cluster == 0 { FatError::BrokenChain } else { FatError::ClusterOutOfRange });
        }
        if steps > total_clusters {
            return Err(FatError::ChainLoop);
        }
        let end = written.checked_add(cluster_size).ok_or(FatError::BufferTooSmall)?;
        let slot = out.get_mut(written..end).ok_or(FatError::BufferTooSmall)?;
        read_cluster(cluster, slot)?;
        written = end;
        steps += 1;
        let next = fat_entry(fat, cluster, kind)?;
        if is_end_of_chain(kind, next) {
            return Ok(written);
        }
        if is_bad_cluster(kind, next) {
            return Err(FatError::BadCluster);
        }
        cluster = next;
    }
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

#[cfg(test)]
mod chain_tests {
    use super::{FatError, FatKind, fat_entry};
    

    #[test]
    fn fat12_even_entries_take_the_low_twelve_bits() {
        // index 0 → 偏移 0 → 两字节 0xF8,0xFF → LE = 0xFFF8 → 偶项取低 12 位 = 0xFF8。
        let fat = std::vec![0xF8u8, 0xFF, 0xFF, 0xFF];
        assert_eq!(fat_entry(&fat, 0, FatKind::Fat12).expect("读取成功"), 0x0FF8);
    }

    #[test]
    fn fat12_odd_entries_shift_across_a_byte_boundary() {
        // index 1 → 偏移 1 + 0 = 1 → 两字节 0xFF,0x0F → LE = 0x0FFF → 奇项右移 4 位 = 0x0FF。
        let fat = std::vec![0xF8u8, 0xFF, 0x0F, 0x00];
        assert_eq!(fat_entry(&fat, 1, FatKind::Fat12).expect("读取成功"), 0x0FF);
    }

    #[test]
    fn fat16_entries_are_little_endian() {
        let fat = std::vec![0x34u8, 0x12, 0x78, 0x56];
        assert_eq!(fat_entry(&fat, 0, FatKind::Fat16).expect("读取成功"), 0x1234);
        assert_eq!(fat_entry(&fat, 1, FatKind::Fat16).expect("读取成功"), 0x5678);
    }

    #[test]
    fn fat32_entries_mask_the_reserved_high_bits() {
        // 高 4 位是保留位，必须屏蔽：全 0xFF 应得到 0x0FFF_FFFF 而不是 0xFFFF_FFFF。
        let fat = std::vec![0xFFu8; 8];
        assert_eq!(fat_entry(&fat, 0, FatKind::Fat32).expect("读取成功"), 0x0FFF_FFFF);
    }

    #[test]
    fn an_entry_past_the_table_is_rejected() {
        let fat = std::vec![0u8; 4];
        assert_eq!(fat_entry(&fat, 100, FatKind::Fat16), Err(FatError::TableOutOfBounds));
        assert_eq!(fat_entry(&fat, u32::MAX, FatKind::Fat12), Err(FatError::TableOutOfBounds));
    }
}

#[cfg(test)]
mod dir_tests {
    use super::{
        Entry, FatError, FatKind, is_bad_cluster, is_end_of_chain, parse_directory,
    };
    

    const SECTOR: usize = 512;

    #[test]
    fn end_of_chain_ranges_differ_per_variant() {
        assert!(is_end_of_chain(FatKind::Fat12, 0x0FF8));
        assert!(is_end_of_chain(FatKind::Fat12, 0x0FFF));
        assert!(!is_end_of_chain(FatKind::Fat12, 0x0FF7), "0xFF7 是坏簇，不是链尾");
        assert!(is_end_of_chain(FatKind::Fat16, 0xFFF8));
        assert!(is_end_of_chain(FatKind::Fat16, 0xFFFF));
        assert!(is_end_of_chain(FatKind::Fat32, 0x0FFF_FFF8));
        assert!(is_end_of_chain(FatKind::Fat32, 0x0FFF_FFFF));
        assert!(!is_end_of_chain(FatKind::Fat16, 5), "普通簇号不是链尾");
    }

    #[test]
    fn bad_clusters_are_distinguished_from_chain_end() {
        assert!(is_bad_cluster(FatKind::Fat12, 0x0FF7));
        assert!(is_bad_cluster(FatKind::Fat16, 0xFFF7));
        assert!(is_bad_cluster(FatKind::Fat32, 0x0FFF_FFF7));
        assert!(!is_bad_cluster(FatKind::Fat12, 0x0FF8), "链尾不是坏簇");
        assert!(!is_bad_cluster(FatKind::Fat32, 0), "空闲簇不是坏簇");
    }

    /// 往扇区里写一个 32 字节目录项。
    fn put_entry(sector: &mut [u8], at: usize, name: &[u8; 11], attributes: u8, cluster: u32, size: u32) {
        sector[at..at + 11].copy_from_slice(name);
        sector[at + 11] = attributes;
        sector[at + 26..at + 28].copy_from_slice(&(cluster as u16).to_le_bytes());
        sector[at + 28..at + 32].copy_from_slice(&size.to_le_bytes());
    }

    #[test]
    fn eight_three_names_are_read_and_the_zero_entry_terminates() {
        let mut sector = std::vec![0u8; SECTOR];
        put_entry(&mut sector, 0, b"HELLO   TXT", 0x20, 5, 1234);
        // 偏移 32 处保持 0 → 表示后续全空。
        let mut out = [Entry::EMPTY; 8];
        let count = parse_directory(&sector, &mut out).expect("解析成功");
        assert_eq!(count, 1);
        assert_eq!(&out[0].name[..], b"HELLO   TXT", "8.3 名以空格填充");
        assert_eq!(out[0].first_cluster, 5);
        assert_eq!(out[0].size, 1234);
    }

    #[test]
    fn deleted_entries_are_skipped_and_long_name_entries_are_skipped() {
        let mut sector = std::vec![0u8; SECTOR];
        // 已删除项：首字节 0xE5。
        put_entry(&mut sector, 0, b"\xE5ONE    TXT", 0x20, 1, 1);
        // 长文件名项：属性 0x0F —— 本实现**明确跳过**（不支持 LFN）。
        put_entry(&mut sector, 32, b"LFN     X  ", 0x0F, 0, 0);
        put_entry(&mut sector, 64, b"KEEP    TXT", 0x20, 7, 42);
        let mut out = [Entry::EMPTY; 8];
        let count = parse_directory(&sector, &mut out).expect("解析成功");
        assert_eq!(count, 1, "已删除项与 LFN 项都不应出现在结果里");
        assert_eq!(&out[0].name[..], b"KEEP    TXT");
        assert_eq!(out[0].first_cluster, 7);
    }

    #[test]
    fn a_short_sector_is_rejected() {
        let sector = std::vec![0u8; 16];
        let mut out = [Entry::EMPTY; 8];
        assert_eq!(parse_directory(&sector, &mut out), Err(FatError::ShortSector));
    }
}

#[cfg(test)]
mod chain_read_tests {
    use super::{FatError, FatKind, read_chain};
    use std::vec::Vec;

    const CLUSTER: usize = 512;

    /// 造一张 FAT16 表：`entries[i]` 是第 i 簇的下一个簇号。
    fn fat16(entries: &[u16]) -> Vec<u8> {
        let mut fat = std::vec![0u8; entries.len() * 2];
        for (index, value) in entries.iter().enumerate() {
            fat[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
        }
        fat
    }

    /// 假簇读取器：按簇号返回一个用该簇号填充的块。
    fn reader(cluster: u32, buffer: &mut [u8]) -> Result<(), FatError> {
        for byte in buffer.iter_mut() {
            *byte = cluster as u8;
        }
        Ok(())
    }

    #[test]
    fn a_normal_chain_is_read_in_order() {
        // 簇 2 → 3 → 4 → 链尾。
        let fat = fat16(&[0, 0, 3, 4, 0xFFFF]);
        let mut out = std::vec![0u8; 3 * CLUSTER];
        let written = read_chain(&fat, 2, FatKind::Fat16, 8, CLUSTER, &mut out, reader).expect("读取成功");
        assert_eq!(written, 3 * CLUSTER);
        assert_eq!(out[0], 2);
        assert_eq!(out[CLUSTER], 3);
        assert_eq!(out[2 * CLUSTER], 4);
    }

    #[test]
    fn a_bad_cluster_stops_with_an_error() {
        // 簇 2 → 0xFFF7（坏簇）。
        let fat = fat16(&[0, 0, 0xFFF7]);
        let mut out = std::vec![0u8; 4 * CLUSTER];
        assert_eq!(
            read_chain(&fat, 2, FatKind::Fat16, 8, CLUSTER, &mut out, reader),
            Err(FatError::BadCluster)
        );
    }

    #[test]
    fn a_free_cluster_means_a_broken_chain() {
        // 簇 2 → 0（空闲）：链断裂，不是正常结束。
        let fat = fat16(&[0, 0, 0]);
        let mut out = std::vec![0u8; 4 * CLUSTER];
        assert_eq!(
            read_chain(&fat, 2, FatKind::Fat16, 8, CLUSTER, &mut out, reader),
            Err(FatError::BrokenChain)
        );
    }

    #[test]
    fn a_loop_in_the_chain_is_rejected_instead_of_spinning_forever() {
        // 簇 2 → 2：自环，必须报错（否则死循环）。
        let fat = fat16(&[0, 0, 2]);
        // 缓冲必须大到让“步数上限”先触发（总簇数 8），否则会先撞 BufferTooSmall。
        let mut out = std::vec![0u8; 16 * CLUSTER];
        assert_eq!(
            read_chain(&fat, 2, FatKind::Fat16, 8, CLUSTER, &mut out, reader),
            Err(FatError::ChainLoop)
            , "自环必须报错，不能原地打转"
        );
    }

    #[test]
    fn a_cluster_beyond_the_volume_is_rejected() {
        // 簇 2 → 99，但卷只有 8 个簇。
        let fat = fat16(&[0, 0, 99]);
        let mut out = std::vec![0u8; 4 * CLUSTER];
        assert_eq!(
            read_chain(&fat, 2, FatKind::Fat16, 8, CLUSTER, &mut out, reader),
            Err(FatError::ClusterOutOfRange)
        );
    }

    #[test]
    fn a_too_small_output_buffer_is_reported() {
        let fat = fat16(&[0, 0, 3, 0xFFFF]);
        let mut out = std::vec![0u8; CLUSTER];
        assert_eq!(
            read_chain(&fat, 2, FatKind::Fat16, 8, CLUSTER, &mut out, reader),
            Err(FatError::BufferTooSmall)
        );
    }
}
