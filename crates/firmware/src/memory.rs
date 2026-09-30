//! 固件提供的内存映射：类型、条目与映射视图。
//!
//! 取值与 Limine 协议的内存映射类型一一对应（`limine.h` 的 `limine_memmap_entry_type`），
//! 因此协议层可以零转换地搬运；未知数值一律返回 `None`（宁可报错，不猜）。

use arch::addr::PhysAddr;

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
}

/// 内存映射：固件给出的条目序列。
#[derive(Clone, Copy)]
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

#[cfg(test)]
mod tests {
    use super::{MemoryEntry, MemoryKind, MemoryMap};
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
}
