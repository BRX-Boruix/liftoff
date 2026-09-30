//! ISO9660 只读（L4）。
//!
//! 边界：**纯字节解析**（只依赖 `core`）；介质内容由调用方读入后交给本模块。
//!
//! 主卷描述符在 **LBA 16**（偏移 32768，2048 字节逻辑块）：类型 @0（1 = 主卷描述符）、
//! 标识 `CD001` @1..6、版本 @6、逻辑块大小 @128（u16 小端）、根目录记录 @156（34 字节）。
//!
//! **双端序**：ISO9660 的数值字段存两份（小端 4 字节 + 大端 4 字节）。本模块**读小端并核对
//! 大端副本** —— 不一致即映像损坏，必须响亮报错（与 GPT 做 CRC 校验同一种思路）。

/// 主卷描述符所在偏移。
pub const ISO9660_DESCRIPTOR_OFFSET: usize = 32768;
/// 卷描述符标识。
pub const ISO9660_ID: [u8; 5] = *b"CD001";
/// 描述符长度（一个逻辑块）。
pub const ISO9660_DESCRIPTOR_SIZE: usize = 2048;
/// 主卷描述符的类型值。
pub const ISO9660_PRIMARY: u8 = 1;

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IsoError {
    /// 映像不足。
    ShortImage,
    /// 标识不是 `CD001`。
    NotIso9660,
    /// 不是主卷描述符。
    NotPrimary,
    /// 逻辑块大小为 0。
    BadBlockSize,
    /// 双端序两份不一致（映像损坏）。
    EndianMismatch,
}

/// 一个目录记录。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirRecord {
    /// 数据起始 LBA。
    pub extent_lba: u32,
    /// 数据长度（字节）。
    pub data_length: u32,
    /// 名字长度。
    pub name_len: u8,
    /// 名字（定长存放，有效范围是前 `name_len` 字节）。
    pub name: [u8; 255],
}

impl DirRecord {
    /// 占位值。
    pub const EMPTY: Self = Self { extent_lba: 0, data_length: 0, name_len: 0, name: [0; 255] };

    /// 名字切片。
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

/// 卷描述符的关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VolumeDescriptor {
    /// 逻辑块大小。
    pub block_size: u16,
    /// 根目录记录。
    pub root: DirRecord,
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

fn read_u32_be(raw: &[u8], at: usize) -> Option<u32> {
    let bytes = raw.get(at..at.checked_add(4)?)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    Some(u32::from_be_bytes(buf))
}

/// 读双端序 u32：小端在前、大端在后，两份必须一致。
///
/// 注意：两份的**字节序不同** —— 小端副本用 `from_le_bytes`、大端副本必须用
/// `from_be_bytes`（曾经这里两份都用小端读，导致永远 `EndianMismatch`）。
fn read_both_u32(raw: &[u8], at: usize) -> Result<u32, IsoError> {
    let le = read_u32(raw, at).ok_or(IsoError::ShortImage)?;
    let be = read_u32_be(raw, at + 4).ok_or(IsoError::ShortImage)?;
    if le != be {
        return Err(IsoError::EndianMismatch);
    }
    Ok(le)
}

/// 解析主卷描述符。
pub fn parse_primary_descriptor(image: &[u8]) -> Result<VolumeDescriptor, IsoError> {
    let end = ISO9660_DESCRIPTOR_OFFSET
        .checked_add(ISO9660_DESCRIPTOR_SIZE)
        .ok_or(IsoError::ShortImage)?;
    let raw = image
        .get(ISO9660_DESCRIPTOR_OFFSET..end)
        .ok_or(IsoError::ShortImage)?;
    if raw.get(1..6) != Some(&ISO9660_ID[..]) {
        return Err(IsoError::NotIso9660);
    }
    if raw[0] != ISO9660_PRIMARY {
        return Err(IsoError::NotPrimary);
    }
    let block_size = read_u16(raw, 128).ok_or(IsoError::ShortImage)?;
    if block_size == 0 {
        return Err(IsoError::BadBlockSize);
    }
    let record = raw.get(156..190).ok_or(IsoError::ShortImage)?;
    let extent_lba = read_both_u32(record, 2)?;
    let data_length = read_both_u32(record, 10)?;
    let name_len = *record.get(32).ok_or(IsoError::ShortImage)?;
    let mut name = [0u8; 255];
    let take = name_len.min(255) as usize;
    let source = record.get(33..33 + take).ok_or(IsoError::ShortImage)?;
    name[..take].copy_from_slice(source);
    Ok(VolumeDescriptor { block_size, root: DirRecord { extent_lba, data_length, name_len, name } })
}

#[cfg(test)]
mod tests {
    use super::{
        ISO9660_DESCRIPTOR_OFFSET, ISO9660_ID, IsoError, parse_primary_descriptor,
    };
    use std::vec::Vec;

    /// 把 u32 写成“双端序”：小端 4 字节 + 大端 4 字节。
    fn put_both_endian(target: &mut [u8], le_at: usize, value: u32) {
        target[le_at..le_at + 4].copy_from_slice(&value.to_le_bytes());
        target[le_at + 4..le_at + 8].copy_from_slice(&value.to_be_bytes());
    }

    /// 造一个映像：32768 字节填充 + 一个主卷描述符扇区。
    fn image(kind: u8, ident: [u8; 5], root_extent: u32, break_endian: bool) -> Vec<u8> {
        let mut raw = std::vec![0u8; ISO9660_DESCRIPTOR_OFFSET + 2048];
        let at = ISO9660_DESCRIPTOR_OFFSET;
        raw[at] = kind;
        raw[at + 1..at + 6].copy_from_slice(&ident);
        raw[at + 6] = 1;
        raw[at + 128..at + 130].copy_from_slice(&2048u16.to_le_bytes());
        // 根目录记录 @156：长度@0、extent@2（双端序）、data_length@10（双端序）、name_len@32、name@33
        raw[at + 156] = 34;
        put_both_endian(&mut raw, at + 158, root_extent);
        put_both_endian(&mut raw, at + 166, 4096);
        if break_endian {
            // 只破坏大端副本，制造不一致。
            raw[at + 158 + 4..at + 158 + 8].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        }
        raw[at + 188] = 1;
        raw[at + 189] = 0;
        raw
    }

    #[test]
    fn a_valid_descriptor_yields_the_root_directory() {
        let raw = image(1, ISO9660_ID, 20, false);
        let descriptor = parse_primary_descriptor(&raw).expect("描述符有效");
        assert_eq!(descriptor.block_size, 2048);
        assert_eq!(descriptor.root.extent_lba, 20);
        assert_eq!(descriptor.root.data_length, 4096);
        assert_eq!(descriptor.root.name_len, 1, "根目录记录的名字是单字节 0x00");
    }

    #[test]
    fn a_bad_identifier_is_rejected() {
        let raw = image(1, *b"CD002", 20, false);
        assert_eq!(parse_primary_descriptor(&raw), Err(IsoError::NotIso9660));
    }

    #[test]
    fn a_non_primary_descriptor_is_rejected() {
        // 255 = 卷描述符集合终止符，不是主卷描述符。
        let raw = image(255, ISO9660_ID, 20, false);
        assert_eq!(parse_primary_descriptor(&raw), Err(IsoError::NotPrimary));
    }

    #[test]
    fn a_double_endian_mismatch_is_rejected() {
        // ISO9660 的数值字段存两份（小端+大端）；不一致说明映像损坏。
        let raw = image(1, ISO9660_ID, 20, true);
        assert_eq!(parse_primary_descriptor(&raw), Err(IsoError::EndianMismatch));
    }

    #[test]
    fn a_short_image_is_rejected() {
        let raw = std::vec![0u8; 1024];
        assert_eq!(parse_primary_descriptor(&raw), Err(IsoError::ShortImage));
    }
}