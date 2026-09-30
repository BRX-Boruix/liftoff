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
    /// 目录记录长度非法（< 33 或越出块尾）。
    BadRecordLength,
    /// 目录记录的名字长度超出其记录范围。
    BadNameLength,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
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

/// 一个目录记录。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    /// 数据起始 LBA。
    pub extent_lba: u32,
    /// 数据长度（字节）。
    pub data_length: u32,
    /// 标志位（bit1 = 目录）。
    pub flags: u8,
    /// 名字长度。
    pub name_len: u8,
    /// 名字（定长存放，避免分配）。
    pub name: [u8; 255],
}

impl Entry {
    /// 占位值。
    pub const EMPTY: Self = Self { extent_lba: 0, data_length: 0, flags: 0, name_len: 0, name: [0; 255] };

    /// 名字切片。
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

/// 遍历一个逻辑块里的目录记录，返回条目数。
///
/// **关键边界**：`len == 0` 表示**本逻辑块结束** → 立即停止（否则游标原地不动会**死循环**）。
/// `len < 33` 或 `at + len` 越出块尾视为损坏；`name_len` 必须落在 `len - 33` 之内。
/// 记录里的双端序两份都要按各自字节序读并核对。
pub fn parse_directory_block(block: &[u8], out: &mut [Entry]) -> Result<usize, IsoError> {
    let mut count = 0;
    let mut at = 0usize;
    loop {
        let len = match block.get(at) {
            Some(value) => *value as usize,
            None => break,
        };
        if len == 0 {
            break;
        }
        if len < 33 {
            return Err(IsoError::BadRecordLength);
        }
        let end = at.checked_add(len).ok_or(IsoError::BadRecordLength)?;
        if end > block.len() {
            return Err(IsoError::BadRecordLength);
        }
        let record = block.get(at..end).ok_or(IsoError::BadRecordLength)?;
        let extent_lba = read_both_u32(record, 2)?;
        let data_length = read_both_u32(record, 10)?;
        let flags = *record.get(25).ok_or(IsoError::ShortImage)?;
        let name_len = *record.get(32).ok_or(IsoError::ShortImage)? as usize;
        if name_len > len - 33 {
            return Err(IsoError::BadNameLength);
        }
        if count == out.len() {
            return Err(IsoError::BufferTooSmall);
        }
        let mut entry = Entry::EMPTY;
        entry.extent_lba = extent_lba;
        entry.data_length = data_length;
        entry.flags = flags;
        entry.name_len = name_len as u8;
        let source = record.get(33..33 + name_len).ok_or(IsoError::ShortImage)?;
        entry.name[..name_len].copy_from_slice(source);
        out[count] = entry;
        count += 1;
        at = end;
    }
    Ok(count)
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

#[cfg(test)]
mod dir_tests {
    use super::{Entry, IsoError, parse_directory_block};
    

    const BLOCK: usize = 2048;

    fn put_both_endian(target: &mut [u8], le_at: usize, value: u32) {
        target[le_at..le_at + 4].copy_from_slice(&value.to_le_bytes());
        target[le_at + 4..le_at + 8].copy_from_slice(&value.to_be_bytes());
    }

    /// 往块里写一条目录记录；返回该记录占用的长度。
    fn put_record(block: &mut [u8], at: usize, extent: u32, length: u32, name: &[u8]) -> usize {
        let record_len = 33 + name.len();
        block[at] = record_len as u8;
        put_both_endian(block, at + 2, extent);
        put_both_endian(block, at + 10, length);
        block[at + 25] = 2;
        block[at + 32] = name.len() as u8;
        block[at + 33..at + 33 + name.len()].copy_from_slice(name);
        record_len
    }

    #[test]
    fn records_are_walked_and_the_zero_length_terminates_the_block() {
        let mut block = std::vec![0u8; BLOCK];
        let first = put_record(&mut block, 0, 20, 2048, &[0x00]);
        let _second = put_record(&mut block, first, 20, 2048, &[0x01]);
        // 偏移 first+second 处已是 0 → 表示本块结束。
        let mut out = [Entry::EMPTY; 8];
        let count = parse_directory_block(&block, &mut out).expect("解析成功");
        assert_eq!(count, 2, "len == 0 必须终止遍历（不能原地打转）");
        assert_eq!(out[0].extent_lba, 20);
        assert_eq!(out[0].data_length, 2048);
        assert_eq!(out[1].name_len, 1);
        assert_eq!(out[1].name[0], 0x01, "名字 0x01 表示上级目录");
    }

    #[test]
    fn an_empty_block_yields_no_entries() {
        let block = std::vec![0u8; BLOCK];
        let mut out = [Entry::EMPTY; 8];
        assert_eq!(parse_directory_block(&block, &mut out).expect("解析成功"), 0);
    }

    #[test]
    fn a_record_length_past_the_block_is_rejected() {
        // 用 100 字节的小块：len = 200 必然越界（2048 字节块里 200 并不越界）。
        let mut block = std::vec![0u8; 100];
        block[0] = 200;
        block[32] = 1;
        let mut out = [Entry::EMPTY; 8];
        assert_eq!(parse_directory_block(&block, &mut out), Err(IsoError::BadRecordLength));
    }

    #[test]
    fn a_name_length_past_its_record_is_rejected() {
        let mut block = std::vec![0u8; BLOCK];
        block[0] = 34;
        block[32] = 200;
        let mut out = [Entry::EMPTY; 8];
        assert_eq!(parse_directory_block(&block, &mut out), Err(IsoError::BadNameLength));
    }

    #[test]
    fn a_double_endian_mismatch_in_a_record_is_rejected() {
        let mut block = std::vec![0u8; BLOCK];
        put_record(&mut block, 0, 20, 2048, &[0x00]);
        block[2 + 4..2 + 8].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        let mut out = [Entry::EMPTY; 8];
        assert_eq!(parse_directory_block(&block, &mut out), Err(IsoError::EndianMismatch));
    }
}
