//! 从固件内存映射里提取可分配的物理区间。
//!
//! 计入的类型与理由：
//! - `Usable`：常规可用内存 ✓；
//! - `BootloaderReclaimable`：退出引导服务后即可回收 ✓ —— 但**本引导器自己正在使用的**那部分
//!   必须先由调用方从映射里标走（本函数无法知道哪些是自用）✓，故这条理由写在此处以免误用。
//!
//! 结果按起始地址升序、并合并相邻或重叠区间；全过程**不分配**（原地插入排序 + 原地合并）。
//! `base + length` 溢出的区间直接报错（不保留、不合并）—— 这种区间无法安全使用，宁可拒绝。

use arch::addr::PhysAddr;
use firmware::error::Error;
use firmware::memory::{MemoryKind, MemoryMap};

/// 一个可分配的物理区间。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UsableRange {
    /// 起始物理地址。
    pub base: PhysAddr,
    /// 长度（字节）。
    pub length: u64,
}

impl UsableRange {
    /// 结束地址（不含）；加法溢出返回 `None`。
    pub const fn end(self) -> Option<u64> {
        self.base.as_u64().checked_add(self.length)
    }
}

/// 提取可分配区间，写入 `out`，返回区间数。
pub fn usable_ranges(map: MemoryMap<'_>, out: &mut [UsableRange]) -> Result<usize, Error> {
    let mut count = 0;
    for entry in map.iter() {
        if !matches!(
            entry.kind,
            MemoryKind::Usable | MemoryKind::BootloaderReclaimable
        ) {
            continue;
        }
        if entry.length == 0 {
            continue;
        }
        let range = UsableRange { base: entry.base, length: entry.length };
        // 溢出端点的区间无法安全使用：拒绝，而不是保留下来让调用方踩坑。
        if range.end().is_none() {
            return Err(Error::InvalidArgument);
        }
        if count == out.len() {
            return Err(Error::BufferTooSmall);
        }
        // 原地插入排序（规模小，且引导阶段不能分配）。
        let mut at = count;
        while at > 0 && out[at - 1].base.as_u64() > range.base.as_u64() {
            out[at] = out[at - 1];
            at -= 1;
        }
        out[at] = range;
        count += 1;
    }

    // 原地合并相邻或重叠区间。
    let mut merged = 0;
    for i in 0..count {
        let current = out[i];
        if merged > 0 {
            let prev = out[merged - 1];
            if let (Some(prev_end), Some(current_end)) = (prev.end(), current.end()) {
                if current.base.as_u64() <= prev_end {
                    let new_end = if current_end > prev_end { current_end } else { prev_end };
                    out[merged - 1] = UsableRange {
                        base: prev.base,
                        length: new_end - prev.base.as_u64(),
                    };
                    continue;
                }
            }
        }
        out[merged] = current;
        merged += 1;
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::{UsableRange, usable_ranges};
    use arch::addr::PhysAddr;
    use firmware::error::Error;
    use firmware::memory::{MemoryEntry, MemoryKind, MemoryMap};

    fn entry(base: u64, length: u64, kind: MemoryKind) -> MemoryEntry {
        MemoryEntry { base: PhysAddr::new(base), length, kind }
    }

    fn range(base: u64, length: u64) -> UsableRange {
        UsableRange { base: PhysAddr::new(base), length }
    }

    #[test]
    fn only_usable_and_reclaimable_kinds_are_taken() {
        let entries = [
            entry(0x1000, 0x1000, MemoryKind::Reserved),
            entry(0x2000, 0x1000, MemoryKind::Usable),
            entry(0x3000, 0x1000, MemoryKind::AcpiReclaimable),
            entry(0x4000, 0x1000, MemoryKind::BootloaderReclaimable),
        ];
        let mut out = [range(0, 0); 4];
        let count = usable_ranges(MemoryMap::new(&entries), &mut out).expect("提取成功");
        assert_eq!(count, 2);
        assert_eq!(out[0], range(0x2000, 0x1000));
        assert_eq!(out[1], range(0x4000, 0x1000));
    }

    #[test]
    fn ranges_are_sorted_by_base() {
        let entries = [
            entry(0x9000, 0x1000, MemoryKind::Usable),
            entry(0x2000, 0x1000, MemoryKind::Usable),
            entry(0x5000, 0x1000, MemoryKind::Usable),
        ];
        let mut out = [range(0, 0); 4];
        let count = usable_ranges(MemoryMap::new(&entries), &mut out).expect("提取成功");
        assert_eq!(count, 3);
        assert_eq!(out[0].base.as_u64(), 0x2000);
        assert_eq!(out[1].base.as_u64(), 0x5000);
        assert_eq!(out[2].base.as_u64(), 0x9000);
    }

    #[test]
    fn adjacent_and_overlapping_ranges_are_merged() {
        let entries = [
            entry(0x1000, 0x1000, MemoryKind::Usable),
            entry(0x2000, 0x1000, MemoryKind::Usable),
            entry(0x3000, 0x1000, MemoryKind::Usable),
        ];
        let mut out = [range(0, 0); 4];
        let count = usable_ranges(MemoryMap::new(&entries), &mut out).expect("提取成功");
        assert_eq!(count, 1, "三段相邻应合并为一段");
        assert_eq!(out[0], range(0x1000, 0x3000));

        let overlap = [
            entry(0x1000, 0x2000, MemoryKind::Usable),
            entry(0x2000, 0x2000, MemoryKind::Usable),
        ];
        let mut out2 = [range(0, 0); 4];
        let count2 = usable_ranges(MemoryMap::new(&overlap), &mut out2).expect("提取成功");
        assert_eq!(count2, 1, "重叠应合并为一段");
        assert_eq!(out2[0], range(0x1000, 0x3000));
    }

    #[test]
    fn a_too_small_output_buffer_is_reported() {
        let entries = [
            entry(0x1000, 0x1000, MemoryKind::Usable),
            entry(0x9000, 0x1000, MemoryKind::Usable),
        ];
        let mut out = [range(0, 0); 1];
        assert_eq!(usable_ranges(MemoryMap::new(&entries), &mut out), Err(Error::BufferTooSmall));
    }

    #[test]
    fn a_length_overflow_is_rejected() {
        // base + length 溢出：必须报错（保留这种区间会让调用方踩坑）。
        let entries = [
            entry(u64::MAX - 0x1000, 0x2000, MemoryKind::Usable),
            entry(0x0, 0x1000, MemoryKind::Usable),
        ];
        let mut out = [range(0, 0); 4];
        assert_eq!(usable_ranges(MemoryMap::new(&entries), &mut out), Err(Error::InvalidArgument), "溢出端点的区间必须被拒绝，而不是保留");
    }
}