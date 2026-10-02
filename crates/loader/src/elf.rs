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
    /// `ET_DYN` 但没有 `PT_DYNAMIC` 段（无法做重定位）。
    NoDynamicSegment,
    /// 重定位表本身不合法，或含有我们不支持的类型。
    ///
    /// 当前只支持「整张表都是 `R_X86_64_RELATIVE`」这一种情形（由 `DT_RELACOUNT`
    /// 声明）—— **不静默跳过**任何条目。
    BadRelocationTable,
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // 这些消息会经 `KernelPlanError::Elf` 一路走到真机串口（`report_failure`）。
        match self {
            Self::ShortImage => f.write_str("映像不足 64 字节，装不下 ELF 头"),
            Self::NotElf => f.write_str("魔数不是 \\x7FELF"),
            Self::NotElf64 => f.write_str("不是 64 位 ELF"),
            Self::NotLittleEndian => f.write_str("不是小端序"),
            Self::WrongMachine => f.write_str("e_machine 不是 x86-64"),
            Self::UnsupportedType => f.write_str("e_type 既不是可执行文件也不是 PIE"),
            Self::BadProgramHeader => f.write_str("e_phentsize 不是 56"),
            Self::BadSegmentSize => f.write_str("p_filesz 大于 p_memsz（BSS 只能补零，不能缩短）"),
            Self::SegmentOutOfBounds => f.write_str("程序头表或某个段越出映像范围"),
            Self::NoLoadSegments => f.write_str("没有任何 PT_LOAD 段"),
            Self::BufferTooSmall => f.write_str("调用方给的输出缓冲太小"),
            Self::NoDynamicSegment => f.write_str("ET_DYN 但没有 PT_DYNAMIC 段，无法做重定位"),
            Self::BadRelocationTable => f.write_str("重定位表不合法，或含不支持的重定位类型"),
        }
    }
}

#[cfg(test)]
mod elf_error_display_tests {
    use super::ElfError;
    use std::format;
    use std::string::String;
    use std::vec::Vec;

    #[test]
    fn every_elf_error_has_a_distinct_human_readable_message() {
        // 13 个变体，**全部**必须有不重复的消息：两条错误打出同一行文字，
        // 在真机串口上就等于没有诊断信息。
        let mut seen: Vec<String> = Vec::new();
        for err in [
            ElfError::ShortImage,
            ElfError::NotElf,
            ElfError::NotElf64,
            ElfError::NotLittleEndian,
            ElfError::WrongMachine,
            ElfError::UnsupportedType,
            ElfError::BadProgramHeader,
            ElfError::BadSegmentSize,
            ElfError::SegmentOutOfBounds,
            ElfError::NoLoadSegments,
            ElfError::BufferTooSmall,
            ElfError::NoDynamicSegment,
            ElfError::BadRelocationTable,
        ] {
            let text = format!("{err}");
            assert!(!text.is_empty(), "每条错误都必须有消息");
            assert!(!seen.contains(&text), "消息不得重复: {text}");
            seen.push(text);
        }
        assert_eq!(seen.len(), 13, "必须覆盖全部 13 个变体");
    }
}

/// `PT_DYNAMIC`。
pub const PT_DYNAMIC: u32 = 2;
/// 动态表终止项。
pub const DT_NULL: u64 = 0;
/// `DT_RELA` / `DT_RELASZ` / `DT_RELAENT` / `DT_RELACOUNT`。
pub const DT_RELA: u64 = 7;
pub const DT_RELASZ: u64 = 8;
pub const DT_RELAENT: u64 = 9;
pub const DT_RELACOUNT: u64 = 0x6fff_fff9;
/// `R_X86_64_RELATIVE`：`*(r_offset + slide) = slide + r_addend`。
pub const R_X86_64_RELATIVE: u32 = 8;

/// 一条要写进内核内存的重定位（地址与值都已含 `slide`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Relocation {
    /// 目标虚拟地址。
    pub address: u64,
    /// 应写入的 8 字节值。
    pub value: u64,
}

/// `R_X86_64_RELATIVE` 重定位的惰性迭代器（不分配内存）。
#[derive(Clone, Copy, Debug)]
pub struct RelativeRelocations<'a> {
    image: &'a [u8],
    table: usize,
    remaining: usize,
    entry: usize,
    slide: u64,
}

impl Iterator for RelativeRelocations<'_> {
    type Item = Relocation;

    fn next(&mut self) -> Option<Relocation> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let at = self.table;
        self.table += self.entry;
        let r_offset = le_u64(self.image, at)?;
        let r_addend = le_u64(self.image, at + 16)? as i64;
        Some(Relocation {
            address: r_offset.wrapping_add(self.slide),
            value: (r_addend as u64).wrapping_add(self.slide),
        })
    }
}

fn le_u64(raw: &[u8], at: usize) -> Option<u64> {
    let bytes = raw.get(at..at.checked_add(8)?)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    Some(u64::from_le_bytes(buf))
}

fn le_u32(raw: &[u8], at: usize) -> Option<u32> {
    let bytes = raw.get(at..at.checked_add(4)?)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    Some(u32::from_le_bytes(buf))
}

/// 把虚拟地址翻译成文件偏移（只用 `PT_LOAD` 段）。
fn vaddr_to_file_offset(image: &[u8], hdr: &ElfHeader, vaddr: u64) -> Option<usize> {
    for index in 0..hdr.e_phnum as usize {
        let at = hdr.e_phoff as usize + index * hdr.e_phentsize as usize;
        if le_u32(image, at)? != 1 {
            continue; // 只看 PT_LOAD
        }
        let p_offset = le_u64(image, at + 8)?;
        let p_vaddr = le_u64(image, at + 16)?;
        let p_filesz = le_u64(image, at + 32)?;
        let end = p_vaddr.checked_add(p_filesz)?;
        if vaddr >= p_vaddr && vaddr < end {
            let delta = vaddr - p_vaddr;
            let file = p_offset.checked_add(delta)?;
            return usize::try_from(file).ok();
        }
    }
    None
}

/// 解析 `ET_DYN` 映像的 `R_X86_64_RELATIVE` 重定位表。
///
/// `slide` = 实际装载地址 − 链接期虚拟地址（按链接地址装载时为 0）。
///
/// # 为什么必须做
///
/// PIE 内核的数据段里存的是**链接期地址**，而 `.rela.dyn` 保存的是「应该写什么」。
/// 不应用重定位，那些槽位就保持文件里的 0 —— 实测后果：内核 `_start` 从
/// `0xffffffff809b0550` 读自己的栈指针，读到 0，`mov %rcx,%rsp` 后立刻压栈崩溃。
pub fn relative_relocations(image: &[u8], slide: u64) -> Result<RelativeRelocations<'_>, ElfError> {
    let hdr = parse_elf_header(image)?;
    if hdr.e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }
    // 找 PT_DYNAMIC。
    let mut dynamic: Option<(usize, usize)> = None;
    for index in 0..hdr.e_phnum as usize {
        let at = hdr.e_phoff as usize + index * hdr.e_phentsize as usize;
        if le_u32(image, at).ok_or(ElfError::BadProgramHeader)? != PT_DYNAMIC {
            continue;
        }
        let p_offset = le_u64(image, at + 8).ok_or(ElfError::BadProgramHeader)?;
        let p_filesz = le_u64(image, at + 32).ok_or(ElfError::BadProgramHeader)?;
        dynamic = Some((
            usize::try_from(p_offset).map_err(|_| ElfError::BadProgramHeader)?,
            usize::try_from(p_filesz).map_err(|_| ElfError::BadProgramHeader)?,
        ));
        break;
    }
    let (dyn_at, dyn_len) = dynamic.ok_or(ElfError::NoDynamicSegment)?;
    // 遍历动态表，收集需要的 DT_*。
    let mut rela_vaddr: Option<u64> = None;
    let mut rela_size: u64 = 0;
    let mut rela_ent: u64 = 24;
    let mut rela_count: Option<u64> = None;
    let entries = dyn_len / 16;
    for index in 0..entries {
        let at = dyn_at + index * 16;
        let tag = le_u64(image, at).ok_or(ElfError::BadRelocationTable)?;
        let val = le_u64(image, at + 8).ok_or(ElfError::BadRelocationTable)?;
        match tag {
            DT_NULL => break,
            DT_RELA => rela_vaddr = Some(val),
            DT_RELASZ => rela_size = val,
            DT_RELAENT => rela_ent = val,
            DT_RELACOUNT => rela_count = Some(val),
            _ => {}
        }
    }
    let rela_vaddr = rela_vaddr.ok_or(ElfError::BadRelocationTable)?;
    if rela_ent != 24 || rela_size == 0 || rela_size % rela_ent != 0 {
        return Err(ElfError::BadRelocationTable);
    }
    let count = rela_size / rela_ent;
    // 只支持「整张表都是 RELATIVE」：由 DT_RELACOUNT 明确声明。
    if rela_count != Some(count) {
        return Err(ElfError::BadRelocationTable);
    }
    let table = vaddr_to_file_offset(image, &hdr, rela_vaddr).ok_or(ElfError::BadRelocationTable)?;
    let end = table.checked_add(usize::try_from(rela_size).map_err(|_| ElfError::BadRelocationTable)?)
        .ok_or(ElfError::BadRelocationTable)?;
    if end > image.len() {
        return Err(ElfError::BadRelocationTable);
    }
    Ok(RelativeRelocations {
        image,
        table,
        remaining: usize::try_from(count).map_err(|_| ElfError::BadRelocationTable)?,
        entry: 24,
        slide,
    })
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
/// 按节名返回某节的**文件区间** `[sh_offset, sh_offset + sh_size)`。
///
/// **为什么需要**：固件环境里内存访问极慢（量级估计每次读约 100 微秒 ✓），对全映像或
/// 全段的顺序扫描都不可行；而引导器要找的 Limine 请求是内核的**静态数据**，实测全部落在
/// 真实内核的 `.data` 节内 —— 用节表定位它，把扫描范围缩小约 300 倍（真实内核对照测试
/// 在 limine 协议层：扫 `.data` 与扫全映像得到同样的 7 个请求）。
///
/// 边界：节名表越界、名字不是 UTF-8、`sh_size` 溢出，一律返回 `None`，**不 panic**。
pub fn section_file_range(image: &[u8], wanted: &str) -> Option<(usize, usize)> {
    if image.len() < 64 || &image[0..4] != ELF_MAGIC {
        return None;
    }
    let rd_u16 = |image: &[u8], at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(image.get(at..at + 2)?.try_into().ok()?))
    };
    let rd_u32 = |image: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().ok()?))
    };
    let rd_u64 = |image: &[u8], at: usize| -> Option<u64> {
        Some(u64::from_le_bytes(image.get(at..at + 8)?.try_into().ok()?))
    };
    let shoff = rd_u64(image, 0x28)? as usize;
    let shentsize = rd_u16(image, 0x3A)? as usize;
    let shnum = rd_u16(image, 0x3C)? as usize;
    let shstrndx = rd_u16(image, 0x3E)? as usize;
    if shentsize < 64 || shentsize > 512 {
        return None;
    }
    let read_sh = |index: usize| -> Option<(u32, usize, usize)> {
        let at = shoff.checked_add(index.checked_mul(shentsize)?)?;
        if at + 64 > image.len() {
            return None;
        }
        Some((
            rd_u32(image, at)?,
            rd_u64(image, at + 24)? as usize,
            rd_u64(image, at + 32)? as usize,
        ))
    };
    let (_, strtab_offset, _) = read_sh(shstrndx)?;
    for index in 0..shnum {
        let (name_off, offset, size) = read_sh(index)?;
        let name_at = strtab_offset.checked_add(name_off as usize)?;
        let mut end = name_at;
        while image.get(end) != Some(&0) {
            end += 1;
        }
        let name = image.get(name_at..end)?;
        if name == wanted.as_bytes() {
            let size = size;
            let end = offset.checked_add(size)?;
            if end > image.len() {
                return None;
            }
            return Some((offset, end));
        }
    }
    None
}

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
mod real_artifact_tests {
    use super::{ET_DYN, EM_X86_64, ProgramHeader, parse_elf_header, parse_load_segments};

    /// 用**真实内核产物**验证解析与段布局（不是自造夹具）。
    ///
    /// 产物不在时**明确跳过并说明**（不让测试假装通过）。
    #[test]
    fn the_real_kernel_artifact_matches_the_measured_layout() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../kernel/target/x86_64-unknown-none/release/kernel"
        );
        let Ok(image) = std::fs::read(path) else {
            std::eprintln!("跳过：真实内核产物不存在（{path}）");
            return;
        };
        let header = parse_elf_header(&image).expect("真实内核应可解析");
        assert_eq!(header.e_machine, EM_X86_64);
        assert_eq!(header.e_type, ET_DYN, "实测为 ET_DYN（PIE）");
        assert_eq!(header.e_entry, 0xffff_ffff_8001_af20, "实测入口");
        assert_eq!(header.e_phentsize as usize, 56);
        assert_eq!(header.e_phnum, 4);

        let mut out = [ProgramHeader::EMPTY; 8];
        let count = parse_load_segments(&image, &header, &mut out).expect("段可解析");
        assert_eq!(count, 3, "实测 3 个 PT_LOAD");
        assert_eq!(out[0].p_vaddr, 0xffff_ffff_8000_0000);
        assert_eq!(out[0].p_filesz, 0xba478);
        assert_eq!(out[0].p_memsz, 0xba478, "实测 filesz == memsz（无 BSS）");
        assert_eq!(out[1].p_vaddr, 0xffff_ffff_800b_b000);
        assert_eq!(out[2].p_vaddr, 0xffff_ffff_804e_c000);
        // 跳转前检查依赖这一点：入口必须落在某个已装载段内。
        assert!(
            header.e_entry >= out[0].p_vaddr && header.e_entry < out[0].p_vaddr + out[0].p_memsz,
            "入口必须落在第 0 段内"
        );
    }
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

#[cfg(test)]
mod section_range_tests {
    use super::section_file_range;

    /// 构造一个最小 ELF：ELF 头 + 节名表 + 两个节头（.text、.data）。
    fn minimal_elf() -> std::vec::Vec<u8> {
        let mut image = std::vec![0u8; 4096];
        image[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        image[4] = 2;   // 64 位
        image[5] = 1;   // 小端
        // e_shoff @0x28、e_shentsize @0x3A、e_shnum @0x3C、e_shstrndx @0x3E
        let shoff: u64 = 256;
        let shentsize: u16 = 64;
        let shnum: u16 = 3; // 0 空 + .text + .data
        let shstrndx: u16 = 1; // 节名表本身作为第 1 个节头
        image[0x28..0x30].copy_from_slice(&shoff.to_le_bytes());
        image[0x3A..0x3C].copy_from_slice(&shentsize.to_le_bytes());
        image[0x3C..0x3E].copy_from_slice(&shnum.to_le_bytes());
        image[0x3E..0x40].copy_from_slice(&shstrndx.to_le_bytes());
        // 节名表内容放在 **1024 起**：此前放在 384、与节头表（256 起）相邻，
        // 而「节 1 = 名表自身」的节头字段把名表内容覆盖了（sh_name 的 4 字节
        // 落在名表首字节上）—— 名表绝不与节头表或任何节的数据重叠。
        let names = b"\x00.text\x00.data\x00";
        image[1024..1024 + names.len()].copy_from_slice(names);
        fn write_sh(image: &mut [u8], shoff: usize, shentsize: usize, index: usize, name: u32, offset: u64, size: u64) {
            let at = shoff + index * shentsize;
            image[at..at + 4].copy_from_slice(&name.to_le_bytes());
            image[at + 24..at + 32].copy_from_slice(&offset.to_le_bytes());
            image[at + 32..at + 40].copy_from_slice(&size.to_le_bytes());
        }
        // 节 1 = 节名表（内容在 1024 起，sh_name = 0 指向空名）
        write_sh(&mut image, shoff as usize, shentsize as usize, 1, 0, 1024, 16);
        // 节 2 = .text（名字在名字表里的偏移 1）
        write_sh(&mut image, shoff as usize, shentsize as usize, 2, 1, 1280, 64);
        // shnum = 4：0 空 + 名表 + .text + .data
        image[0x3C..0x3E].copy_from_slice(&4u16.to_le_bytes());
        // 节 3 = .data（名字偏移 7）
        write_sh(&mut image, shoff as usize, shentsize as usize, 3, 7, 2048, 128);
        image
    }

    #[test]
    fn a_section_is_located_by_name_with_its_file_range() {
        let image = minimal_elf();
        let (start, end) = section_file_range(&image, ".data").expect("应找到 .data");
        assert_eq!((start, end), (2048, 2048 + 128));
        let (tstart, tend) = section_file_range(&image, ".text").expect("应找到 .text");
        assert_eq!((tstart, tend), (1280, 1280 + 64));
    }

    #[test]
    fn a_missing_section_is_none() {
        let image = minimal_elf();
        // 只断言「不存在的节名 → None」。空名字的匹配对象（节 0 与名表自身的空名）
        // 在协议里没有意义，不在此规定行为。
        assert_eq!(section_file_range(&image, ".rodata"), None);
    }
}
#[cfg(test)]
mod relocation_tests {
    extern crate std;

    use super::{relative_relocations, Relocation};
    use std::vec::Vec;

    /// 真实内核在 ISO 里的位置（extent LBA 33、24,619,400 字节）。
    const KERNEL_LBA: usize = 33;
    const BLOCK: usize = 2048;
    const KERNEL_BYTES: usize = 24_619_400;

    fn real_kernel() -> Vec<u8> {
        // 用**我们实际装载的那份内核**（ISO 里的 `/boot/kernel`），不是内核仓库的构建产物。
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let iso = std::fs::read(path).expect("读 boruix.iso（真实产物）");
        let start = KERNEL_LBA * BLOCK;
        iso[start..start + KERNEL_BYTES].to_vec()
    }

    /// 真实内核是 `ET_DYN`，`DT_RELACOUNT` 声明整张 `.rela.dyn` 都是
    /// `R_X86_64_RELATIVE` —— 条数与大小必须严格对上，不能靠猜。
    #[test]
    fn real_kernel_rela_table_is_entirely_relative() {
        let image = real_kernel();
        let all: Vec<Relocation> = relative_relocations(&image, 0)
            .expect("解析真实内核的重定位表")
            .collect();
        assert_eq!(all.len(), 17_706, "DT_RELACOUNT = 0x452a");
    }

    /// **这条重定位就是内核启动失败的直接原因**：内核 `_start` 从
    /// `0xffffffff809b0550` 读自己的栈指针，文件里是 0，只有应用重定位
    /// 才会变成 `0xffffffff809af000`。
    #[test]
    fn real_kernel_relocates_the_stack_pointer_global() {
        let image = real_kernel();
        let hit = relative_relocations(&image, 0)
            .expect("解析真实内核的重定位表")
            .find(|r| r.address == 0xffff_ffff_809b_0550)
            .expect("必须存在 0xffffffff809b0550 的重定位");
        assert_eq!(hit.value, 0xffff_ffff_809a_f000, "r_addend = -0x7f651000");
    }

    /// `slide` 必须同时加到目标地址与写入值上。
    #[test]
    fn slide_is_applied_to_both_address_and_value() {
        let image = real_kernel();
        let hit = relative_relocations(&image, 0x1000)
            .expect("解析真实内核的重定位表")
            .find(|r| r.address == 0xffff_ffff_809b_1550)
            .expect("带 slide 时目标地址也要平移");
        assert_eq!(hit.value, 0xffff_ffff_809a_f000 + 0x1000);
    }

    /// 非 `ET_DYN` 映像不该走重定位路径。
    #[test]
    fn rejects_non_dyn_images() {
        let mut image = real_kernel();
        image[16] = 2; // e_type = ET_EXEC
        assert!(relative_relocations(&image, 0).is_err());
    }
}
