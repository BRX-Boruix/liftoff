//! ELF64 解析（L4）。
//!
//! 边界：**纯字节解析**（只依赖 `core`）；映像内容由 `fs` 层读出后交给本模块。
//!
//! 只接受：64 位（`EI_CLASS` = 2）、小端（`EI_DATA` = 1）、x86-64（`e_machine` = 62）、
//! `e_type` ∈ {`ET_EXEC`, `ET_DYN`}。`ET_DYN`（PIE 内核）**接受**，但其**重定位**
//! 属于后续步骤，本模块不声称已解决。
//!
//! ELF64 头布局：`e_ident[16]` @0（`EI_CLASS`@4、`EI_DATA`@5）、`e_type`@16（u16）、
//! `e_machine`@18（u16）、`e_entry`@24（u64）、`e_phoff`@32（u64）、`e_phentsize`@54（u16）、
//! `e_phnum`@56（u16）；头长 64 字节。

/// ELF 魔数。
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
/// x86-64 机器类型。
pub const EM_X86_64: u16 = 62;
/// 可执行文件。
pub const ET_EXEC: u16 = 2;
/// 位置无关可执行文件（PIE）。
pub const ET_DYN: u16 = 3;
/// ELF64 头长度。
pub const ELF64_HEADER_SIZE: usize = 64;

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElfError {
    /// 映像不足 64 字节。
    ShortImage,
    /// 魔数不是 `\x7FELF`。
    NotElf,
    /// 不是 64 位 ELF。
    NotElf64,
    /// 不是小端。
    NotLittleEndian,
    /// 不是 x86-64。
    WrongMachine,
    /// `e_type` 不是可执行文件或 PIE。
    UnsupportedType,
}

/// ELF64 头的关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ElfHeader {
    /// `e_type`。
    pub e_type: u16,
    /// `e_machine`。
    pub e_machine: u16,
    /// 入口地址。
    pub e_entry: u64,
    /// 程序头表偏移。
    pub e_phoff: u64,
    /// 程序头大小。
    pub e_phentsize: u16,
    /// 程序头数量。
    pub e_phnum: u16,
}

fn read_u16(raw: &[u8], at: usize) -> Option<u16> {
    let bytes = raw.get(at..at.checked_add(2)?)?;
    let mut buf = [0u8; 2];
    buf.copy_from_slice(bytes);
    Some(u16::from_le_bytes(buf))
}

fn read_u64(raw: &[u8], at: usize) -> Option<u64> {
    let bytes = raw.get(at..at.checked_add(8)?)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    Some(u64::from_le_bytes(buf))
}

/// 解析 ELF64 头。
pub fn parse_elf_header(image: &[u8]) -> Result<ElfHeader, ElfError> {
    let raw = image.get(..ELF64_HEADER_SIZE).ok_or(ElfError::ShortImage)?;
    if raw.get(0..4) != Some(&ELF_MAGIC[..]) {
        return Err(ElfError::NotElf);
    }
    if raw[4] != 2 {
        return Err(ElfError::NotElf64);
    }
    if raw[5] != 1 {
        return Err(ElfError::NotLittleEndian);
    }
    let e_type = read_u16(raw, 16).ok_or(ElfError::ShortImage)?;
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }
    let e_machine = read_u16(raw, 18).ok_or(ElfError::ShortImage)?;
    if e_machine != EM_X86_64 {
        return Err(ElfError::WrongMachine);
    }
    Ok(ElfHeader {
        e_type,
        e_machine,
        e_entry: read_u64(raw, 24).ok_or(ElfError::ShortImage)?,
        e_phoff: read_u64(raw, 32).ok_or(ElfError::ShortImage)?,
        e_phentsize: read_u16(raw, 54).ok_or(ElfError::ShortImage)?,
        e_phnum: read_u16(raw, 56).ok_or(ElfError::ShortImage)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ELF_MAGIC, EM_X86_64, ET_DYN, ET_EXEC, ElfError, parse_elf_header,
    };
    use std::vec::Vec;

    /// 造一个 ELF64 头（64 字节）。
    fn header(class: u8, data: u8, kind: u16, machine: u16) -> Vec<u8> {
        let mut raw = std::vec![0u8; 64];
        raw[0..4].copy_from_slice(&ELF_MAGIC);
        raw[4] = class;
        raw[5] = data;
        raw[6] = 1;
        raw[16..18].copy_from_slice(&kind.to_le_bytes());
        raw[18..20].copy_from_slice(&machine.to_le_bytes());
        raw[24..32].copy_from_slice(&0xffff_ffff_8000_0000u64.to_le_bytes());
        raw[32..40].copy_from_slice(&64u64.to_le_bytes());
        raw[52..54].copy_from_slice(&64u16.to_le_bytes());
        raw[54..56].copy_from_slice(&56u16.to_le_bytes());
        raw[56..58].copy_from_slice(&3u16.to_le_bytes());
        raw
    }

    #[test]
    fn a_valid_header_yields_its_fields() {
        let raw = header(2, 1, ET_EXEC, EM_X86_64);
        let head = parse_elf_header(&raw).expect("头有效");
        assert_eq!(head.e_type, ET_EXEC);
        assert_eq!(head.e_machine, EM_X86_64);
        assert_eq!(head.e_entry, 0xffff_ffff_8000_0000);
        assert_eq!(head.e_phoff, 64);
        assert_eq!(head.e_phentsize, 56);
        assert_eq!(head.e_phnum, 3);
    }

    #[test]
    fn a_pie_kernel_is_accepted() {
        let raw = header(2, 1, ET_DYN, EM_X86_64);
        assert_eq!(parse_elf_header(&raw).expect("ET_DYN 可接受").e_type, ET_DYN);
    }

    #[test]
    fn a_bad_magic_is_rejected() {
        let mut raw = header(2, 1, ET_EXEC, EM_X86_64);
        raw[1] = b'X';
        assert_eq!(parse_elf_header(&raw), Err(ElfError::NotElf));
    }

    #[test]
    fn a_32_bit_elf_is_rejected() {
        let raw = header(1, 1, ET_EXEC, EM_X86_64);
        assert_eq!(parse_elf_header(&raw), Err(ElfError::NotElf64));
    }

    #[test]
    fn a_big_endian_elf_is_rejected() {
        let raw = header(2, 2, ET_EXEC, EM_X86_64);
        assert_eq!(parse_elf_header(&raw), Err(ElfError::NotLittleEndian));
    }

    #[test]
    fn another_architecture_is_rejected() {
        let raw = header(2, 1, ET_EXEC, 0xB7);
        assert_eq!(parse_elf_header(&raw), Err(ElfError::WrongMachine));
    }

    #[test]
    fn an_unsupported_type_is_rejected() {
        // ET_REL(1) 是重定位目标文件，不是可执行内核。
        let raw = header(2, 1, 1, EM_X86_64);
        assert_eq!(parse_elf_header(&raw), Err(ElfError::UnsupportedType));
    }

    #[test]
    fn a_short_image_is_rejected() {
        let raw = std::vec![0u8; 32];
        assert_eq!(parse_elf_header(&raw), Err(ElfError::ShortImage));
    }
}