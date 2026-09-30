//! 固件提供的内存映射：类型、条目与映射视图。
//!
//! 取值与 Limine 协议的内存映射类型一一对应（`limine.h` 的 `limine_memmap_entry_type`），
//! 因此协议层可以零转换地搬运；未知数值一律返回 `None`（宁可报错，不猜）。

use arch::addr::PhysAddr;

use crate::error::Error;

/// 内存区类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum MemoryKind {
    /// 可用内存。
    Usable = 0,
    /// 保留。
    Reserved = 1,
    /// ACPI 可回收。
    AcpiReclaimable = 2,
    /// ACPI NVS。
    AcpiNvs = 3,
    /// 坏内存。
    BadMemory = 4,
    /// 引导器可回收。
    BootloaderReclaimable = 5,
    /// 内核与模块占用。
    KernelAndModules = 6,
}

impl MemoryKind {
    /// 由协议数值构造；未知值返回 `None`。
    #[inline]
    pub const fn from_protocol(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Usable),
            1 => Some(Self::Reserved),
            2 => Some(Self::AcpiReclaimable),
            3 => Some(Self::AcpiNvs),
            4 => Some(Self::BadMemory),
            5 => Some(Self::BootloaderReclaimable),
            6 => Some(Self::KernelAndModules),
            _ => None,
        }
    }

    /// 协议数值。
    #[inline]
    pub const fn as_protocol(self) -> u32 {
        self as u32
    }
}

/// 一个内存区。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoryEntry {
    /// 起始物理地址。
    pub base: PhysAddr,
    /// 长度（字节）。
    pub length: u64,
    /// 类型。
    pub kind: MemoryKind,
}

impl MemoryEntry {
    /// 结束物理地址（不含）；加法溢出返回 `None`。
    #[inline]
    pub const fn end(self) -> Option<PhysAddr> {
        self.base.checked_add(self.length)
    }

    /// 是否为空区。
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.length == 0
    }
    /// 由固件原始三元组构造；未知类型返回 `Error::Unsupported`（不猜）。
    pub fn from_raw(base: PhysAddr, length: u64, protocol_kind: u32) -> Result<Self, Error> {
        let kind = MemoryKind::from_protocol(protocol_kind).ok_or(Error::Unsupported)?;
        Ok(Self { base, length, kind })
    }
}

/// 内存映射：固件给出的条目序列。
#[derive(Clone, Copy, Debug)]
pub struct MemoryMap<'a> {
    entries: &'a [MemoryEntry],
}

impl<'a> MemoryMap<'a> {
    /// 由条目切片构造。
    #[inline]
    pub const fn new(entries: &'a [MemoryEntry]) -> Self {
        Self { entries }
    }

    /// 全部条目。
    #[inline]
    pub const fn entries(&self) -> &'a [MemoryEntry] {
        self.entries
    }

    /// 迭代条目。
    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'a, MemoryEntry> {
        self.entries.iter()
    }

    /// 条目数。
    #[inline]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空映射。
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
/// 把 `source` 的条目复制进 `buffer`；容量不足返回 `Error::BufferTooSmall`。
///
/// 宿主测试与各固件实现共用（单点定义）：缓冲由调用方拥有，符合 no_std 下无隐式分配的约定。
pub fn copy_map<'b>(buffer: &'b mut [MemoryEntry], source: &[MemoryEntry]) -> Result<MemoryMap<'b>, Error> {
    if source.len() > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    buffer[..source.len()].copy_from_slice(source);
    Ok(MemoryMap::new(&buffer[..source.len()]))
}

/// 内存映射来源：固件层能力 trait 之一。
///
/// 分成小 trait 而非巨 trait：BIOS 可只实现它支持的部分，测试替身也可按需实现。
pub trait MemoryMapSource {
    /// 取内存映射；条目数超过 `buffer` 容量时返回 `Error::BufferTooSmall`。
    fn memory_map<'b>(&mut self, buffer: &'b mut [MemoryEntry]) -> Result<MemoryMap<'b>, Error>;
}

#[cfg(test)]
mod tests {
    use super::{MemoryEntry, MemoryKind, MemoryMap, MemoryMapSource, copy_map};
    use arch::addr::PhysAddr;

    #[test]
    fn memory_kind_round_trips_the_protocol_value() {
        let all = [
            (MemoryKind::Usable, 0u32),
            (MemoryKind::Reserved, 1),
            (MemoryKind::AcpiReclaimable, 2),
            (MemoryKind::AcpiNvs, 3),
            (MemoryKind::BadMemory, 4),
            (MemoryKind::BootloaderReclaimable, 5),
            (MemoryKind::KernelAndModules, 6),
        ];
        for (kind, value) in all {
            assert_eq!(kind.as_protocol(), value);
            assert_eq!(MemoryKind::from_protocol(value), Some(kind));
        }
    }

    #[test]
    fn memory_kind_rejects_unknown_protocol_values() {
        assert_eq!(MemoryKind::from_protocol(7), None);
        assert_eq!(MemoryKind::from_protocol(u32::MAX), None);
    }

    #[test]
    fn entry_end_address_reports_overflow() {
        let entry = MemoryEntry { base: PhysAddr::new(0x1000), length: 0x1000, kind: MemoryKind::Usable };
        assert_eq!(entry.end().map(|a| a.as_u64()), Some(0x2000));
        let overflow = MemoryEntry { base: PhysAddr::new(u64::MAX), length: 1, kind: MemoryKind::Usable };
        assert_eq!(overflow.end(), None);
    }

    #[test]
    fn empty_entry_is_detected() {
        let empty = MemoryEntry { base: PhysAddr::new(0), length: 0, kind: MemoryKind::Reserved };
        assert!(empty.is_empty());
    }

    #[test]
    fn map_exposes_its_entries() {
        let entries = [
            MemoryEntry { base: PhysAddr::new(0), length: 0x1000, kind: MemoryKind::Usable },
            MemoryEntry { base: PhysAddr::new(0x1000), length: 0x1000, kind: MemoryKind::Reserved },
        ];
        let map = MemoryMap::new(&entries);
        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
        assert_eq!(map.iter().count(), 2);
        assert_eq!(map.entries()[1].kind, MemoryKind::Reserved);
        let empty = MemoryMap::new(&[]);
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn entry_from_raw_rejects_unknown_firmware_kind() {
        use crate::error::Error;
        let entry = MemoryEntry::from_raw(PhysAddr::new(0x1000), 0x1000, 0).expect("可用内存");
        assert_eq!(entry.kind, MemoryKind::Usable);
        assert_eq!(MemoryEntry::from_raw(PhysAddr::new(0), 1, 7), Err(Error::Unsupported));
    }

    #[test]
    fn copy_map_reports_buffer_too_small() {
        use crate::error::Error;
        let source = [
            MemoryEntry { base: PhysAddr::new(0), length: 0x1000, kind: MemoryKind::Usable },
            MemoryEntry { base: PhysAddr::new(0x1000), length: 0x1000, kind: MemoryKind::Reserved },
        ];
        let mut small = [MemoryEntry { base: PhysAddr::new(0), length: 0, kind: MemoryKind::Usable }; 1];
        assert!(matches!(copy_map(&mut small, &source), Err(Error::BufferTooSmall)));
        let mut big = [MemoryEntry { base: PhysAddr::new(0), length: 0, kind: MemoryKind::Usable }; 4];
        let map = copy_map(&mut big, &source).expect("容量足够");
        assert_eq!(map.len(), 2);
        assert_eq!(map.entries()[1].kind, MemoryKind::Reserved);
    }

    #[test]
    fn memory_map_source_is_implementable() {
        struct Fake { entries: [MemoryEntry; 1] }
        impl MemoryMapSource for Fake {
            fn memory_map<'b>(&mut self, buffer: &'b mut [MemoryEntry]) -> Result<MemoryMap<'b>, crate::error::Error> {
                copy_map(buffer, &self.entries)
            }
        }
        let mut fake = Fake {
            entries: [MemoryEntry { base: PhysAddr::new(0x2000), length: 0x1000, kind: MemoryKind::AcpiReclaimable }],
        };
        let mut buffer = [MemoryEntry { base: PhysAddr::new(0), length: 0, kind: MemoryKind::Usable }; 2];
        let map = fake.memory_map(&mut buffer).expect("映射可取");
        assert_eq!(map.entries()[0].kind, MemoryKind::AcpiReclaimable);
    }
}
