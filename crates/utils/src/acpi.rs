//! ACPI 表访问与 **MADT** 解析（S1，对标 brxLimine `lib/acpi.c` + `common/sys/smp.c` 的枚举部分）。
//!
//! **边界**：本模块只做「字节流 → 结构」的**纯解析**，不碰固件、不碰 LAPIC、不分配。
//! 因此它**宿主可测** —— 而 AP 启动的其余部分只能靠真机，把可测的部分先测透是划算的。
//!
//! **不静默吞掉坏数据**：长度为零的条目、越界的条目、长度不足的条目都**停止遍历并如实报错**，
//! 而不是继续读下去（brxLimine `smp.c:172-175` 正是为此加的守卫）。

/// ACPI 表头（SDT）长度。
pub const SDT_HEADER_LEN: usize = 36;

/// MADT 固定头长度：SDT 头 + Local APIC 地址(4) + Flags(4)。
pub const MADT_HEADER_LEN: usize = 44;

/// MADT 签名。
pub const MADT_SIGNATURE: &[u8; 4] = b"APIC";

/// 条目类型：Processor Local APIC（xAPIC）。
pub const ENTRY_LOCAL_APIC: u8 = 0;
/// 条目类型：Processor Local x2APIC。
pub const ENTRY_LOCAL_X2APIC: u8 = 9;

/// MADT 条目里 flags 的「该 CPU 可用」位。
pub const LAPIC_ENABLED: u32 = 1 << 0;

/// 解析失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AcpiError {
    /// 映像不足，装不下固定头。
    ShortTable,
    /// 表长度字段小于固定头，或越出映像范围。
    BadLength,
    /// 签名不匹配。
    WrongSignature,
    /// 某个条目声明长度为 0（继续读会死循环）。
    ZeroLengthEntry,
    /// 某个条目越出表尾。
    EntryOutOfBounds,
    /// 条目长度小于该类型的最小长度。
    ShortEntry,
}

impl core::fmt::Display for AcpiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ShortTable => f.write_str("映像不足，装不下 ACPI 表头"),
            Self::BadLength => f.write_str("ACPI 表长度字段不合法"),
            Self::WrongSignature => f.write_str("ACPI 表签名不匹配"),
            Self::ZeroLengthEntry => f.write_str("MADT 条目长度为 0（继续读会死循环）"),
            Self::EntryOutOfBounds => f.write_str("MADT 条目越出表尾"),
            Self::ShortEntry => f.write_str("MADT 条目长度小于该类型要求"),
        }
    }
}

/// MADT 描述的一个处理器。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MadtCpu {
    /// ACPI 处理器 ID（**不是** LAPIC ID）。
    pub processor_id: u32,
    /// LAPIC ID：xAPIC 为 8 位、x2APIC 为 32 位。
    pub apic_id: u32,
    /// 固件是否声明该 CPU **可用**（flags bit 0）。
    pub enabled: bool,
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(slice);
    Some(u32::from_le_bytes(buf))
}

fn u64_le(bytes: &[u8], at: usize) -> Option<u64> {
    let slice = bytes.get(at..at + 8)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(slice);
    Some(u64::from_le_bytes(buf))
}

/// 取任意 SDT 的**表长度**字段并校验它自洽（不检查签名 —— 那是调用方的事）。
fn sdt_length(sdt: &[u8]) -> Result<usize, AcpiError> {
    if sdt.len() < SDT_HEADER_LEN {
        return Err(AcpiError::ShortTable);
    }
    let length = u32_le(sdt, 4).ok_or(AcpiError::ShortTable)? as usize;
    if length < SDT_HEADER_LEN || length > sdt.len() {
        return Err(AcpiError::BadLength);
    }
    Ok(length)
}

/// 取 MADT 的表长度字段，并校验它与签名、与映像范围自洽。
///
/// 返回的是**表自身的长度**（而不是 `table.len()`）：固件给的缓冲可能比表大。
pub fn table_length(table: &[u8]) -> Result<usize, AcpiError> {
    if table.len() < SDT_HEADER_LEN {
        return Err(AcpiError::ShortTable);
    }
    if table.get(0..4) != Some(MADT_SIGNATURE.as_slice()) {
        return Err(AcpiError::WrongSignature);
    }
    let length = u32_le(table, 4).ok_or(AcpiError::ShortTable)? as usize;
    if length < MADT_HEADER_LEN || length > table.len() {
        return Err(AcpiError::BadLength);
    }
    Ok(length)
}

/// 根表的类型 —— **项宽不同**（RSDT 4 字节、XSDT 8 字节），所以必须显式区分。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SdtKind {
    /// RSDT（ACPI 1.0）：4 字节项。
    Rsdt,
    /// XSDT（ACPI 2.0+）：8 字节项。
    Xsdt,
}

/// RSDP 解析结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RsdpTables {
    /// 根表类型。
    pub kind: SdtKind,
    /// 根表（RSDT / XSDT）的**物理地址**。
    pub root: u64,
}

/// RSDP 签名（8 字节，含空格）。
pub const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// 解析 RSDP，得到根表地址与类型。
///
/// **按修订号选根表**：修订号 ≥ 2 才有 XSDT 字段（偏移 24）。ACPI 1.0 的 RSDP 只有 20 字节，
/// 去读偏移 24 会读到**别的东西** —— 所以必须按修订号与长度双重判断，不能只看有没有那段内存。
pub fn parse_rsdp(rsdp: &[u8]) -> Result<RsdpTables, AcpiError> {
    if rsdp.len() < 20 {
        return Err(AcpiError::ShortTable);
    }
    if rsdp.get(0..8) != Some(RSDP_SIGNATURE.as_slice()) {
        return Err(AcpiError::WrongSignature);
    }
    if rsdp[15] >= 2 {
        // XSDT 字段在偏移 24 —— 需要 36 字节，否则会读到别的东西。
        if rsdp.len() < 36 {
            return Err(AcpiError::ShortTable);
        }
        let root = u64_le(rsdp, 24).ok_or(AcpiError::ShortTable)?;
        Ok(RsdpTables { kind: SdtKind::Xsdt, root })
    } else {
        let root = u32_le(rsdp, 16).ok_or(AcpiError::ShortTable)? as u64;
        Ok(RsdpTables { kind: SdtKind::Rsdt, root })
    }
}

/// 在根表里按**签名**查找子表，返回其物理地址。
///
/// 根表项里只有**地址**、没有签名，所以必须**读每张表的头 4 字节**来比对 —— 这也是本函数
/// 需要一个读取器的原因：读取器由调用方给（真机是物理内存，宿主测试是假数据）。
pub fn find_table<R>(
    sdt: &[u8],
    kind: SdtKind,
    signature: &[u8; 4],
    mut read_header: R,
) -> Result<Option<u64>, AcpiError>
where
    R: FnMut(u64, &mut [u8; 4]) -> bool,
{
    let length = sdt_length(sdt)?;
    let entry_size = match kind {
        SdtKind::Rsdt => 4usize,
        SdtKind::Xsdt => 8usize,
    };
    let mut at = SDT_HEADER_LEN;
    while at + entry_size <= length {
        let address = match kind {
            SdtKind::Rsdt => u32_le(sdt, at).ok_or(AcpiError::ShortTable)? as u64,
            SdtKind::Xsdt => u64_le(sdt, at).ok_or(AcpiError::ShortTable)?,
        };
        let mut header = [0u8; 4];
        if read_header(address, &mut header) && &header == signature {
            return Ok(Some(address));
        }
        at += entry_size;
    }
    Ok(None)
}

/// 把 MADT 里描述的处理器写进 `out`，返回**表里声明的总数**。
///
/// **返回总数而不是写入数**：`out` 太小时调用方能据此发现「漏了 CPU」，而不是静默截断。
/// 实际写入的是 `min(总数, out.len())` 条。
pub fn cpus(table: &[u8], out: &mut [MadtCpu]) -> Result<usize, AcpiError> {
    let length = table_length(table)?;
    let mut total = 0usize;
    let mut at = MADT_HEADER_LEN;
    while at + 2 <= length {
        let entry_type = table[at];
        let entry_len = table[at + 1] as usize;
        if entry_len == 0 {
            return Err(AcpiError::ZeroLengthEntry);
        }
        let end = at.checked_add(entry_len).ok_or(AcpiError::EntryOutOfBounds)?;
        if end > length {
            return Err(AcpiError::EntryOutOfBounds);
        }
        let entry = &table[at..end];
        match entry_type {
            ENTRY_LOCAL_APIC => {
                // type(1) len(1) processor_id(1) apic_id(1) flags(4)
                if entry.len() < 8 {
                    return Err(AcpiError::ShortEntry);
                }
                let flags = u32_le(entry, 4).ok_or(AcpiError::ShortEntry)?;
                if total < out.len() {
                    out[total] = MadtCpu {
                        processor_id: entry[2] as u32,
                        apic_id: entry[3] as u32,
                        enabled: flags & LAPIC_ENABLED != 0,
                    };
                }
                total += 1;
            }
            ENTRY_LOCAL_X2APIC => {
                // type(1) len(1) reserved(2) x2apic_id(4) flags(4) uid(4)
                if entry.len() < 16 {
                    return Err(AcpiError::ShortEntry);
                }
                let apic_id = u32_le(entry, 4).ok_or(AcpiError::ShortEntry)?;
                let flags = u32_le(entry, 8).ok_or(AcpiError::ShortEntry)?;
                if total < out.len() {
                    out[total] = MadtCpu {
                        processor_id: u32_le(entry, 12).ok_or(AcpiError::ShortEntry)?,
                        apic_id,
                        enabled: flags & LAPIC_ENABLED != 0,
                    };
                }
                total += 1;
            }
            _ => {}
        }
        at = end;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// 造一张 MADT：固定头 + 给定条目。
    fn madt(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut table = std::vec![0u8; MADT_HEADER_LEN];
        table[0..4].copy_from_slice(MADT_SIGNATURE);
        for entry in entries {
            table.extend_from_slice(entry);
        }
        let length = table.len() as u32;
        table[4..8].copy_from_slice(&length.to_le_bytes());
        table
    }

    fn xapic(processor_id: u8, apic_id: u8, enabled: bool) -> Vec<u8> {
        let flags: u32 = if enabled { LAPIC_ENABLED } else { 0 };
        let mut entry = std::vec![0u8; 8];
        entry[0] = ENTRY_LOCAL_APIC;
        entry[1] = 8;
        entry[2] = processor_id;
        entry[3] = apic_id;
        entry[4..8].copy_from_slice(&flags.to_le_bytes());
        entry
    }

    fn x2apic(x2apic_id: u32, uid: u32, enabled: bool) -> Vec<u8> {
        let flags: u32 = if enabled { LAPIC_ENABLED } else { 0 };
        let mut entry = std::vec![0u8; 16];
        entry[0] = ENTRY_LOCAL_X2APIC;
        entry[1] = 16;
        entry[4..8].copy_from_slice(&x2apic_id.to_le_bytes());
        entry[8..12].copy_from_slice(&flags.to_le_bytes());
        entry[12..16].copy_from_slice(&uid.to_le_bytes());
        entry
    }

    const EMPTY_CPU: MadtCpu = MadtCpu { processor_id: 0, apic_id: 0, enabled: false };

    fn rsdp(revision: u8, rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut table = std::vec![0u8; 36];
        table[0..8].copy_from_slice(RSDP_SIGNATURE);
        table[15] = revision;
        table[16..20].copy_from_slice(&rsdt.to_le_bytes());
        table[20..24].copy_from_slice(&36u32.to_le_bytes());
        table[24..32].copy_from_slice(&xsdt.to_le_bytes());
        table
    }

    #[test]
    fn rsdp_revision_selects_the_root_table() {
        // **只看"有没有那段内存"是不够的**：ACPI 1.0 的 RSDP 只有 20 字节，
        // 偏移 24 处是**别的东西**。必须按修订号选。
        assert_eq!(
            parse_rsdp(&rsdp(0, 0x1234_5000, 0xDEAD_BEEF)),
            Ok(RsdpTables { kind: SdtKind::Rsdt, root: 0x1234_5000 }),
            "修订 0（ACPI 1.0）必须用 RSDT"
        );
        assert_eq!(
            parse_rsdp(&rsdp(2, 0x1234_5000, 0xABCD_0000)),
            Ok(RsdpTables { kind: SdtKind::Xsdt, root: 0xABCD_0000 }),
            "修订 2 必须用 XSDT"
        );
    }

    #[test]
    fn a_wrong_rsdp_signature_is_rejected() {
        let mut bad = rsdp(2, 0, 0);
        bad[0] = 88; // ASCII X
        assert_eq!(parse_rsdp(&bad), Err(AcpiError::WrongSignature));
        assert_eq!(parse_rsdp(&[0u8; 8]), Err(AcpiError::ShortTable));
    }

    #[test]
    fn find_table_matches_on_the_signature_it_reads_back() {
        // 根表项只有地址、没有签名 —— 必须读每张表的头 4 字节比对。
        // 造一张 XSDT，两项：0x1000 是别的表，0x2000 是 MADT。
        // **这里我第一次写错了**：只分配了 `MADT_HEADER_LEN`(44) 字节，却往 `44..52` 写
        // 第二项 —— 越界 panic。XSDT 的项是 8 字节，两项需要 44+16 字节。
        let mut sdt = std::vec![0u8; MADT_HEADER_LEN + 16];
        sdt[0..4].copy_from_slice(b"XSDT");
        sdt[36..44].copy_from_slice(&0x1000u64.to_le_bytes());
        sdt[44..52].copy_from_slice(&0x2000u64.to_le_bytes());
        let length = sdt.len() as u32;
        sdt[4..8].copy_from_slice(&length.to_le_bytes());

        let mut reader = |address: u64, out: &mut [u8; 4]| {
            if address == 0x2000 {
                out.copy_from_slice(MADT_SIGNATURE);
                true
            } else {
                out.copy_from_slice(b"XXXX");
                true
            }
        };
        assert_eq!(
            find_table(&sdt, SdtKind::Xsdt, MADT_SIGNATURE, &mut reader),
            Ok(Some(0x2000u64))
        );
    }

    #[test]
    fn find_table_reports_none_when_the_signature_is_absent() {
        let mut sdt = std::vec![0u8; MADT_HEADER_LEN + 8];
        sdt[0..4].copy_from_slice(b"XSDT");
        sdt[36..44].copy_from_slice(&0x1000u64.to_le_bytes());
        let length = sdt.len() as u32;
        sdt[4..8].copy_from_slice(&length.to_le_bytes());
        let mut reader = |_a: u64, out: &mut [u8; 4]| {
            out.copy_from_slice(b"XXXX");
            true
        };
        assert_eq!(find_table(&sdt, SdtKind::Xsdt, MADT_SIGNATURE, &mut reader), Ok(None));
    }

    #[test]
    fn parses_xapic_entries_and_honours_the_enabled_flag() {
        let table = madt(&[xapic(0, 0, true), xapic(1, 1, true), xapic(2, 2, false)]);
        let mut out = [EMPTY_CPU; 4];
        assert_eq!(cpus(&table, &mut out), Ok(3));
        assert_eq!(out[0], MadtCpu { processor_id: 0, apic_id: 0, enabled: true });
        assert_eq!(out[1], MadtCpu { processor_id: 1, apic_id: 1, enabled: true });
        // 不可用的 CPU **仍要报告**，只是 enabled=false —— 由调用方决定要不要启动它。
        assert_eq!(out[2], MadtCpu { processor_id: 2, apic_id: 2, enabled: false });
    }

    #[test]
    fn parses_x2apic_entries_with_a_32_bit_id() {
        let table = madt(&[x2apic(0x1234_5678, 7, true)]);
        let mut out = [EMPTY_CPU; 2];
        assert_eq!(cpus(&table, &mut out), Ok(1));
        assert_eq!(out[0].apic_id, 0x1234_5678);
        assert_eq!(out[0].processor_id, 7);
        assert!(out[0].enabled);
    }

    #[test]
    fn unknown_entry_types_are_skipped_by_their_declared_length() {
        // 未知类型必须按**它自己声明的长度**跳过，否则会把后面的 CPU 读成垃圾。
        let mut unknown = std::vec![0u8; 12];
        unknown[0] = 42;
        unknown[1] = 12;
        let table = madt(&[unknown, xapic(0, 3, true)]);
        let mut out = [EMPTY_CPU; 2];
        assert_eq!(cpus(&table, &mut out), Ok(1));
        assert_eq!(out[0].apic_id, 3);
    }

    #[test]
    fn a_zero_length_entry_is_an_error_not_an_infinite_loop() {
        let mut bad = std::vec![0u8; 2];
        bad[0] = 42;
        bad[1] = 0;
        let table = madt(&[bad]);
        let mut out = [EMPTY_CPU; 2];
        assert_eq!(cpus(&table, &mut out), Err(AcpiError::ZeroLengthEntry));
    }

    #[test]
    fn an_entry_overrunning_the_table_is_an_error() {
        let mut bad = std::vec![0u8; 8];
        bad[0] = ENTRY_LOCAL_APIC;
        bad[1] = 200;
        let table = madt(&[bad]);
        let mut out = [EMPTY_CPU; 2];
        assert_eq!(cpus(&table, &mut out), Err(AcpiError::EntryOutOfBounds));
    }

    #[test]
    fn a_truncated_table_and_a_wrong_signature_are_rejected() {
        let mut out = [EMPTY_CPU; 2];
        assert_eq!(cpus(&[0u8; 8], &mut out), Err(AcpiError::ShortTable));
        let mut wrong = madt(&[]);
        wrong[0] = 88; // ASCII X：签名不匹配
        assert_eq!(cpus(&wrong, &mut out), Err(AcpiError::WrongSignature));
        let mut short_len = madt(&[]);
        short_len[4..8].copy_from_slice(&10u32.to_le_bytes());
        assert_eq!(cpus(&short_len, &mut out), Err(AcpiError::BadLength));
    }

    #[test]
    fn a_too_small_output_reports_the_total_so_nothing_is_silently_dropped() {
        // **关键**：`out` 太小必须能被发现。返回总数（3）而不是写入数（1），
        // 调用方一比就知道漏了 CPU。静默截断会让多核少启动几个而无人察觉。
        let table = madt(&[xapic(0, 0, true), xapic(1, 1, true), xapic(2, 2, true)]);
        let mut out = [EMPTY_CPU; 1];
        assert_eq!(cpus(&table, &mut out), Ok(3));
        assert_eq!(out[0].apic_id, 0);
    }
}
