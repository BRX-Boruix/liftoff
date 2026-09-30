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
    /// `e_phentsize` 不是 56。
    BadProgramHeader,
    /// `p_filesz > p_memsz`（BSS 只能补零，不能缩短）。
    BadSegmentSize,
    /// 程序头表或某个段越出映像范围。
    SegmentOutOfBounds,
    /// 没有任何 `PT_LOAD` 段（内核必须至少有一个）。
    NoLoadSegments,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
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

fn read_u32(raw: &[u8], at: usize) -> Option<u32> {
    let bytes = raw.get(at..at.checked_add(4)?)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    Some(u32::from_le_bytes(buf))
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

/// `PT_LOAD`：可装载段。
pub const PT_LOAD: u32 = 1;
/// ELF64 程序头长度。
pub const ELF64_PHDR_SIZE: usize = 56;

/// ELF64 程序头的关键字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProgramHeader {
    /// `p_type`。
    pub p_type: u32,
    /// `p_flags`。
    pub p_flags: u32,
    /// 在映像中的偏移。
    pub p_offset: u64,
    /// 目标虚拟地址。
    pub p_vaddr: u64,
    /// 文件中要拷贝的字节数。
    pub p_filesz: u64,
    /// 内存中占用的字节数（超出 `p_filesz` 的部分是 BSS，须清零）。
    pub p_memsz: u64,
}

impl ProgramHeader {
    /// 占位值。
    pub const EMPTY: Self = Self {
        p_type: 0,
        p_flags: 0,
        p_offset: 0,
        p_vaddr: 0,
        p_filesz: 0,
        p_memsz: 0,
    };
}

/// 解析所有 `PT_LOAD` 程序头，返回数量。
///
/// 数值边界：表范围 `e_phoff + e_phnum × 56` 与每个段的 `p_offset + p_filesz`
/// 都用 checked 运算，越界一律报错（不越界读）；`p_filesz > p_memsz` 视为损坏。
pub fn parse_load_segments(
    image: &[u8],
    header: &ElfHeader,
    out: &mut [ProgramHeader],
) -> Result<usize, ElfError> {
    if header.e_phentsize as usize != ELF64_PHDR_SIZE {
        return Err(ElfError::BadProgramHeader);
    }
    let start = usize::try_from(header.e_phoff).map_err(|_| ElfError::SegmentOutOfBounds)?;
    let total = (header.e_phnum as usize)
        .checked_mul(ELF64_PHDR_SIZE)
        .ok_or(ElfError::SegmentOutOfBounds)?;
    let end = start.checked_add(total).ok_or(ElfError::SegmentOutOfBounds)?;
    let table = image.get(start..end).ok_or(ElfError::SegmentOutOfBounds)?;
    let mut count = 0;
    for index in 0..header.e_phnum as usize {
        let at = index.checked_mul(ELF64_PHDR_SIZE).ok_or(ElfError::SegmentOutOfBounds)?;
        let raw = table.get(at..at + ELF64_PHDR_SIZE).ok_or(ElfError::SegmentOutOfBounds)?;
        let p_type = read_u32(raw, 0).ok_or(ElfError::SegmentOutOfBounds)?;
        if p_type != PT_LOAD {
            continue;
        }
        let p_offset = read_u64(raw, 8).ok_or(ElfError::SegmentOutOfBounds)?;
        let p_filesz = read_u64(raw, 32).ok_or(ElfError::SegmentOutOfBounds)?;
        let p_memsz = read_u64(raw, 40).ok_or(ElfError::SegmentOutOfBounds)?;
        if p_filesz > p_memsz {
            return Err(ElfError::BadSegmentSize);
        }
        let file_end = p_offset.checked_add(p_filesz).ok_or(ElfError::SegmentOutOfBounds)?;
        if file_end > image.len() as u64 {
            return Err(ElfError::SegmentOutOfBounds);
        }
        if count == out.len() {
            return Err(ElfError::BufferTooSmall);
        }
        out[count] = ProgramHeader {
            p_type,
            p_flags: read_u32(raw, 4).ok_or(ElfError::SegmentOutOfBounds)?,
            p_offset,
            p_vaddr: read_u64(raw, 16).ok_or(ElfError::SegmentOutOfBounds)?,
            p_filesz,
            p_memsz,
        };
        count += 1;
    }
    if count == 0 {
        return Err(ElfError::NoLoadSegments);
    }
    Ok(count)
}

/// 清零 BSS 时的分块大小（**固定小缓冲**，避免在栈上开 `p_memsz` 那么大的缓冲）。
const ZERO_CHUNK: usize = 512;

/// 把 `PT_LOAD` 段装载到目标地址：先写 `p_filesz` 字节，再把 BSS 尾部清零。
///
/// 返回装载总量（各段 `p_memsz` 之和）。写入器由调用方注入 —— `loader` 不依赖具体内存实现。
/// 任一写入失败**立即上抛**（半装载的映像不可用）；零长度段不产生任何写入。
pub fn load_segments<W>(
    image: &[u8],
    segments: &[ProgramHeader],
    mut write: W,
) -> Result<usize, ElfError>
where
    W: FnMut(u64, &[u8]) -> Result<(), ElfError>,
{
    let zeros = [0u8; ZERO_CHUNK];
    let mut total = 0usize;
    for segment in segments {
        let file_end = segment
            .p_offset
            .checked_add(segment.p_filesz)
            .ok_or(ElfError::SegmentOutOfBounds)?;
        if file_end > image.len() as u64 {
            return Err(ElfError::SegmentOutOfBounds);
        }
        let start = usize::try_from(segment.p_offset).map_err(|_| ElfError::SegmentOutOfBounds)?;
        let len = usize::try_from(segment.p_filesz).map_err(|_| ElfError::SegmentOutOfBounds)?;
        let bytes = image.get(start..start + len).ok_or(ElfError::SegmentOutOfBounds)?;
        if !bytes.is_empty() {
            write(segment.p_vaddr, bytes)?;
        }
        // `p_filesz <= p_memsz` 已由 `parse_load_segments` 保证，故这里不会下溢。
        let mut remaining = segment.p_memsz - segment.p_filesz;
        let mut at = segment
            .p_vaddr
            .checked_add(segment.p_filesz)
            .ok_or(ElfError::SegmentOutOfBounds)?;
        while remaining > 0 {
            let take = remaining.min(ZERO_CHUNK as u64) as usize;
            write(at, &zeros[..take])?;
            at = at.checked_add(take as u64).ok_or(ElfError::SegmentOutOfBounds)?;
            remaining -= take as u64;
        }
        total += usize::try_from(segment.p_memsz).map_err(|_| ElfError::SegmentOutOfBounds)?;
    }
    Ok(total)
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

#[cfg(test)]
mod phdr_tests {
    use super::{
        ELF_MAGIC, ELF64_PHDR_SIZE, EM_X86_64, ET_EXEC, ElfError, PT_LOAD, parse_elf_header,
        parse_load_segments,
    };
    use std::vec::Vec;

    /// 造一个程序头（56 字节）。
    fn phdr(kind: u32, offset: u64, vaddr: u64, filesz: u64, memsz: u64) -> [u8; 56] {
        let mut raw = [0u8; 56];
        raw[0..4].copy_from_slice(&kind.to_le_bytes());
        raw[4..8].copy_from_slice(&5u32.to_le_bytes());
        raw[8..16].copy_from_slice(&offset.to_le_bytes());
        raw[16..24].copy_from_slice(&vaddr.to_le_bytes());
        raw[32..40].copy_from_slice(&filesz.to_le_bytes());
        raw[40..48].copy_from_slice(&memsz.to_le_bytes());
        raw
    }

    /// 造映像：64 字节头 + 紧随其后的程序头表。
    fn image(headers: &[[u8; 56]]) -> Vec<u8> {
        // 映像 = 64 字节头 + 程序头表 + 4096 字节余量：
        // 余量取大一些，避免段偏移（0x100/0x200 等）算出越界。
        let mut raw = std::vec![0u8; 64 + headers.len() * ELF64_PHDR_SIZE + 4096];
        raw[0..4].copy_from_slice(&ELF_MAGIC);
        raw[4] = 2;
        raw[5] = 1;
        raw[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        raw[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        raw[32..40].copy_from_slice(&64u64.to_le_bytes());
        raw[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        raw[56..58].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        for (index, header) in headers.iter().enumerate() {
            let at = 64 + index * ELF64_PHDR_SIZE;
            raw[at..at + ELF64_PHDR_SIZE].copy_from_slice(header);
        }
        raw
    }

    fn empty_out() -> [super::ProgramHeader; 4] {
        [super::ProgramHeader::EMPTY; 4]
    }

    #[test]
    fn only_load_segments_are_returned() {
        let raw = image(&[
            // 0x100 + 0x100 = 512，远小于映像长度；memsz 0x300 制造 BSS。
            phdr(PT_LOAD, 0x100, 0xffff_ffff_8000_0000, 0x100, 0x300),
            phdr(4, 0, 0, 0, 0),
            phdr(PT_LOAD, 0x200, 0xffff_ffff_8000_1000, 0x100, 0x100),
        ]);
        let head = parse_elf_header(&raw).expect("头有效");
        let mut out = empty_out();
        let count = parse_load_segments(&raw, &head, &mut out).expect("解析成功");
        assert_eq!(count, 2, "PT_NOTE 不应计入");
        assert_eq!(out[0].p_offset, 0x100);
        assert_eq!(out[0].p_vaddr, 0xffff_ffff_8000_0000);
        assert_eq!(out[0].p_filesz, 0x100);
        assert_eq!(out[0].p_memsz, 0x300, "memsz > filesz 表示 BSS");
        assert_eq!(out[1].p_vaddr, 0xffff_ffff_8000_1000);
    }

    #[test]
    fn filesz_greater_than_memsz_is_rejected() {
        let raw = image(&[phdr(PT_LOAD, 0x1000, 0x1000, 0x400, 0x100)]);
        let head = parse_elf_header(&raw).expect("头有效");
        let mut out = empty_out();
        assert_eq!(
            parse_load_segments(&raw, &head, &mut out),
            Err(ElfError::BadSegmentSize)
        );
    }

    #[test]
    fn a_segment_past_the_image_is_rejected() {
        // offset + filesz 越出映像。
        let raw = image(&[phdr(PT_LOAD, 0x9000, 0x1000, 0x100, 0x100)]);
        let head = parse_elf_header(&raw).expect("头有效");
        let mut out = empty_out();
        assert_eq!(
            parse_load_segments(&raw, &head, &mut out),
            Err(ElfError::SegmentOutOfBounds)
        );
    }

    #[test]
    fn a_program_header_table_past_the_image_is_rejected() {
        let mut raw = image(&[phdr(PT_LOAD, 0x1000, 0x1000, 0x100, 0x100)]);
        raw[56..58].copy_from_slice(&100u16.to_le_bytes());
        let head = parse_elf_header(&raw).expect("头有效");
        let mut out = empty_out();
        assert_eq!(
            parse_load_segments(&raw, &head, &mut out),
            Err(ElfError::SegmentOutOfBounds)
        );
    }

    #[test]
    fn an_image_without_load_segments_is_rejected() {
        let raw = image(&[phdr(4, 0, 0, 0, 0)]);
        let head = parse_elf_header(&raw).expect("头有效");
        let mut out = empty_out();
        assert_eq!(
            parse_load_segments(&raw, &head, &mut out),
            Err(ElfError::NoLoadSegments)
        );
    }

    #[test]
    fn a_wrong_program_header_size_is_rejected() {
        let mut raw = image(&[phdr(PT_LOAD, 0x1000, 0x1000, 0x100, 0x100)]);
        raw[54..56].copy_from_slice(&48u16.to_le_bytes());
        let head = parse_elf_header(&raw).expect("头有效");
        let mut out = empty_out();
        assert_eq!(
            parse_load_segments(&raw, &head, &mut out),
            Err(ElfError::BadProgramHeader)
        );
    }
}

#[cfg(test)]
mod load_tests {
    use super::{ElfError, ProgramHeader, load_segments};
    use std::vec::Vec;

    /// 记录写入的写入器。
    struct Recorder {
        writes: Vec<(u64, usize, u8)>,
    }

    impl Recorder {
        fn new() -> Self {
            Self { writes: Vec::new() }
        }

        fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), ElfError> {
            let value = bytes.first().copied().unwrap_or(0);
            self.writes.push((address, bytes.len(), value));
            Ok(())
        }

        fn total(&self) -> usize {
            self.writes.iter().map(|(_, len, _)| *len).sum()
        }
    }

    fn segment(offset: u64, vaddr: u64, filesz: u64, memsz: u64) -> ProgramHeader {
        ProgramHeader {
            p_type: 1,
            p_flags: 5,
            p_offset: offset,
            p_vaddr: vaddr,
            p_filesz: filesz,
            p_memsz: memsz,
        }
    }

    #[test]
    fn file_bytes_are_written_at_the_target_address() {
        // 映像留足余量：64 字节头之后放 0x200 字节的段内容。
        let mut image = std::vec![0u8; 0x400];
        for (index, byte) in image.iter_mut().enumerate().skip(0x100).take(0x100) {
            *byte = (index & 0xFF) as u8;
        }
        let segments = [segment(0x100, 0xffff_ffff_8000_0000, 0x100, 0x100)];
        let mut recorder = Recorder::new();
        let written = load_segments(&image, &segments, |address, bytes| recorder.write(address, bytes)).expect("装载成功");
        assert_eq!(written, 0x100);
        assert_eq!(recorder.writes[0].0, 0xffff_ffff_8000_0000);
        assert_eq!(recorder.total(), 0x100);
    }

    #[test]
    fn the_bss_tail_is_zero_filled() {
        let image = std::vec![0xAAu8; 0x400];
        // filesz = 0x100，memsz = 0x300 → 额外 0x200 字节必须清零。
        let segments = [segment(0x100, 0x1000, 0x100, 0x300)];
        let mut recorder = Recorder::new();
        let written = load_segments(&image, &segments, |address, bytes| recorder.write(address, bytes)).expect("装载成功");
        assert_eq!(written, 0x300, "装载总量应等于 p_memsz");
        assert_eq!(recorder.total(), 0x300);
        let zeroed: usize = recorder.writes.iter().filter(|(_, _, value)| *value == 0).map(|(_, len, _)| *len).sum();
        assert_eq!(zeroed, 0x200, "BSS 尾部必须清零");
    }

    #[test]
    fn a_writer_failure_stops_the_load() {
        let image = std::vec![0u8; 0x400];
        let segments = [segment(0x100, 0x1000, 0x100, 0x100), segment(0x200, 0x2000, 0x100, 0x100)];
        let mut calls = 0;
        let result = load_segments(&image, &segments, |_address, _bytes| {
            calls += 1;
            Err(ElfError::SegmentOutOfBounds)
        });
        assert_eq!(result, Err(ElfError::SegmentOutOfBounds));
        assert_eq!(calls, 1, "失败后不得继续装载后续段");
    }

    #[test]
    fn an_empty_segment_writes_nothing() {
        let image = std::vec![0u8; 0x400];
        let segments = [segment(0, 0x1000, 0, 0)];
        let mut recorder = Recorder::new();
        let written = load_segments(&image, &segments, |address, bytes| recorder.write(address, bytes)).expect("装载成功");
        assert_eq!(written, 0);
        assert!(recorder.writes.is_empty());
    }

    #[test]
    fn a_segment_past_the_image_is_rejected() {
        let image = std::vec![0u8; 0x100];
        let segments = [segment(0x100, 0x1000, 0x100, 0x100)];
        let mut recorder = Recorder::new();
        assert_eq!(
            load_segments(&image, &segments, |address, bytes| recorder.write(address, bytes)),
            Err(ElfError::SegmentOutOfBounds)
        );
    }
}
