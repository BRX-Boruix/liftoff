//! 直接映射区（HHDM）：把物理内存 `[0, top)` 映射到 `offset` 起的虚拟区间。
//!
//! 抽象边界（ADR-007）：换算不假设架构，只依赖本区间的不变量；区间之外一律
//! 返回 `None`（严格模式 S09：宁可报错，绝不返回伪数据）。

use crate::addr::{PhysAddr, VirtAddr};

/// 直接映射区。
///
/// 不变量：`top` 非零且 `offset + top` 不溢出——只能经 [`DirectMap::new`] 构造，
/// 因此换算函数不可能在区间判定上溢出。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirectMap {
    offset: u64,
    top: u64,
}

impl DirectMap {
    /// 由偏移与上界构造；`top == 0` 或 `offset + top` 溢出时返回 `None`。
    #[inline]
    pub const fn new(offset: u64, top: u64) -> Option<Self> {
        if top == 0 {
            return None;
        }
        match offset.checked_add(top) {
            Some(_) => Some(Self { offset, top }),
            None => None,
        }
    }

    /// 虚拟偏移。
    #[inline]
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// 覆盖的物理上界（不含）。
    #[inline]
    pub const fn top(self) -> u64 {
        self.top
    }

    /// 物理地址 → 直接映射虚拟地址；物理地址不在 `[0, top)` 内时返回 `None`。
    #[inline]
    pub const fn phys_to_virt(self, phys: PhysAddr) -> Option<VirtAddr> {
        if phys.as_u64() >= self.top {
            return None;
        }
        match phys.as_u64().checked_add(self.offset) {
            Some(v) => Some(VirtAddr::new(v)),
            None => None,
        }
    }

    /// 直接映射虚拟地址 → 物理地址；不在本区间内时返回 `None`。
    #[inline]
    pub const fn virt_to_phys(self, virt: VirtAddr) -> Option<PhysAddr> {
        match virt.as_u64().checked_sub(self.offset) {
            Some(p) if p < self.top => Some(PhysAddr::new(p)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DirectMap;
    use crate::addr::{PhysAddr, VirtAddr};

    const OFF: u64 = 0xffff_8000_0000_0000;

    #[test]
    fn constructs_only_with_a_valid_range() {
        assert!(DirectMap::new(OFF, 0).is_none());
        assert!(DirectMap::new(u64::MAX, 2).is_none());
        let d = DirectMap::new(OFF, 0x8000_0000).expect("valid range");
        assert_eq!(d.offset(), OFF);
        assert_eq!(d.top(), 0x8000_0000);
    }

    #[test]
    fn phys_to_virt_maps_only_within_the_range() {
        let d = DirectMap::new(OFF, 0x8000_0000).expect("valid range");
        assert_eq!(d.phys_to_virt(PhysAddr::new(0)).map(|v| v.as_u64()), Some(OFF));
        assert_eq!(d.phys_to_virt(PhysAddr::new(0x7FFF_FFFF)).map(|v| v.as_u64()), Some(OFF + 0x7FFF_FFFF));
        assert_eq!(d.phys_to_virt(PhysAddr::new(0x8000_0000)), None);
    }

    #[test]
    fn virt_to_phys_rejects_addresses_outside_the_direct_map() {
        let d = DirectMap::new(OFF, 0x8000_0000).expect("valid range");
        assert_eq!(d.virt_to_phys(VirtAddr::new(OFF)).map(|p| p.as_u64()), Some(0));
        assert_eq!(d.virt_to_phys(VirtAddr::new(OFF + 0x7FFF_FFFF)).map(|p| p.as_u64()), Some(0x7FFF_FFFF));
        assert_eq!(d.virt_to_phys(VirtAddr::new(OFF + 0x8000_0000)), None);
        assert_eq!(d.virt_to_phys(VirtAddr::new(0x1000)), None);
    }

    #[test]
    fn conversions_round_trip() {
        let d = DirectMap::new(OFF, 0x8000_0000).expect("valid range");
        for phys in [0u64, 0x1000, 0x1234_5000, 0x7FFF_FFFF] {
            let virt = d.phys_to_virt(PhysAddr::new(phys)).expect("in range");
            assert_eq!(d.virt_to_phys(virt).map(|p| p.as_u64()), Some(phys));
        }
    }
}
