//! 物理页帧分配：**栈式**。
//!
//! 取舍（写明理由）：引导器**只分配、不释放** → 栈式可做到 O(1) 分配且**零元数据** ✓；
//! 代价是不能回收任意帧 ✗（引导器不需要 ✓）。位图方案能回收任意帧，但要付出
//! “帧数/8”的额外内存与初始化成本 ✗，在这里用不上。
//!
//! 边界：只依赖 `arch`（`PhysAddr`/`PhysFrame`/`PAGE_SIZE`）与 `firmware`；区间由
//! `mm::usable::usable_ranges` 提供（须已排序、已合并、无溢出端点 ✓）。

use crate::usable::UsableRange;
use arch::addr::{PAGE_SIZE, PhysAddr, PhysFrame};

/// 栈式物理页帧分配器。
pub struct StackFrameAllocator<'a> {
    ranges: &'a [UsableRange],
    range_index: usize,
    next: u64,
}

impl<'a> StackFrameAllocator<'a> {
    /// 以可用区间构造（区间须来自 `usable_ranges`）。
    pub const fn new(ranges: &'a [UsableRange]) -> Self {
        Self { ranges, range_index: 0, next: 0 }
    }

    /// 取一帧；耗尽返回 `None`。
    pub fn allocate(&mut self) -> Option<PhysFrame> {
        loop {
            let range = *self.ranges.get(self.range_index)?;
            let base = range.base.as_u64();
            let end = range.end()?;
            if self.next < base {
                self.next = base;
            }
            // 只发**完整**帧：下一个帧必须整帧落在区间内。
            if self.next.checked_add(PAGE_SIZE)? <= end {
                let frame = PhysFrame::containing(PhysAddr::new(self.next));
                self.next += PAGE_SIZE;
                return Some(frame);
            }
            self.range_index += 1;
            self.next = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StackFrameAllocator;
    use crate::usable::UsableRange;
    use arch::addr::{PhysAddr, PAGE_SIZE};

    fn range(base: u64, length: u64) -> UsableRange {
        UsableRange { base: PhysAddr::new(base), length }
    }

    #[test]
    fn frames_are_page_aligned_and_inside_the_ranges() {
        let ranges = [range(0x10_0000, 3 * PAGE_SIZE)];
        let mut allocator = StackFrameAllocator::new(&ranges);
        let first = allocator.allocate().expect("第一帧");
        assert_eq!(first.start_address().expect("地址").as_u64(), 0x10_0000);
        let second = allocator.allocate().expect("第二帧");
        assert_eq!(second.start_address().expect("地址").as_u64(), 0x10_0000 + PAGE_SIZE);
    }

    #[test]
    fn frames_are_never_handed_out_twice() {
        let ranges = [range(0x20_0000, 4 * PAGE_SIZE)];
        let mut allocator = StackFrameAllocator::new(&ranges);
        let mut seen = std::vec::Vec::new();
        while let Some(frame) = allocator.allocate() {
            let addr = frame.start_address().expect("地址").as_u64();
            assert!(!seen.contains(&addr), "重复分配了同一帧");
            seen.push(addr);
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn the_allocator_moves_to_the_next_range_when_one_is_exhausted() {
        let ranges = [range(0x10_0000, PAGE_SIZE), range(0x30_0000, PAGE_SIZE)];
        let mut allocator = StackFrameAllocator::new(&ranges);
        assert_eq!(allocator.allocate().expect("a").start_address().expect("x").as_u64(), 0x10_0000);
        assert_eq!(allocator.allocate().expect("b").start_address().expect("x").as_u64(), 0x30_0000);
        assert!(allocator.allocate().is_none(), "耗尽后必须返回 None");
    }

    #[test]
    fn a_partial_trailing_frame_is_not_handed_out() {
        // 长度不是整页：最后一个不完整帧不能分配。
        let ranges = [range(0x10_0000, PAGE_SIZE + 1)];
        let mut allocator = StackFrameAllocator::new(&ranges);
        assert!(allocator.allocate().is_some());
        assert!(allocator.allocate().is_none(), "不完整帧不得分配");
    }

    #[test]
    fn an_empty_range_list_yields_nothing() {
        let ranges: [UsableRange; 0] = [];
        let mut allocator = StackFrameAllocator::new(&ranges);
        assert!(allocator.allocate().is_none());
    }
}