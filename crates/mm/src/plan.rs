//! 映射规划：把可用区间按大页粒度对齐，产出映射请求（**纯算术**，不触碰页表）。
//!
//! 边界：本模块只做**规划**；落地（写页表）由 `arch::paging::PageTable` 的实现完成，
//! 具体实现由入口/选择器注入（ADR-050）。
//!
//! 数值边界：起点用 `checked_add` 向上对齐（溢出则跳过该区间 ✓）；每页都要
//! `at + large <= end` 才产出 ✓（不会越界，也不会死循环 ✓）。

use crate::usable::UsableRange;
use arch::addr::{PhysAddr, VirtAddr};
use arch::paging::PageFlags;

/// 一条映射请求。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mapping {
    /// 虚拟地址。
    pub virt: VirtAddr,
    /// 物理地址。
    pub phys: PhysAddr,
    /// 长度（字节）。
    pub len: u64,
    /// 权限。
    pub flags: PageFlags,
}

impl Mapping {
    /// 占位值（便于调用方初始化数组）。
    pub const EMPTY: Self = Self {
        virt: VirtAddr::new(0),
        phys: PhysAddr::new(0),
        len: 0,
        flags: PageFlags::none(),
    };
}

/// 规划失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanError {
    /// 页粒度为零（否则会除零/死循环）。
    InvalidPageSize,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
    /// 虚拟地址计算溢出（绝不回绕）。
    AddressOverflow,
}

impl core::fmt::Display for PlanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // 这些消息会经 `PlanBuildError` 一路走到真机串口（`report_failure`），
        // 所以必须**人类可读**，而不是只有 `Debug` 的变体名。
        match self {
            Self::InvalidPageSize => f.write_str("页粒度为零"),
            Self::BufferTooSmall => f.write_str("调用方给的输出缓冲太小"),
            Self::AddressOverflow => f.write_str("虚拟地址计算溢出"),
        }
    }
}

/// 规划**恒等映射**（`virt == phys`），返回产出的映射条数。
/// 共用规划核心：按 `large` 对齐遍历区间，虚拟地址由 `virt_of` 给出。
///
/// `virt_of` 返回 `None` 表示该页的虚拟地址无法表示（溢出）→ 整段报错，**不静默丢弃**。
fn plan_with(
    ranges: &[UsableRange],
    out: &mut [Mapping],
    large: u64,
    virt_of: impl Fn(u64) -> Option<u64>,
) -> Result<usize, PlanError> {
    if large == 0 {
        return Err(PlanError::InvalidPageSize);
    }
    let mut count = 0;
    for range in ranges {
        let base = range.base.as_u64();
        let end = match range.end() {
            Some(end) => end,
            None => continue,
        };
        let aligned = match base.checked_add(large - 1) {
            Some(value) => value & !(large - 1),
            None => continue,
        };
        let mut at = aligned;
        while at.checked_add(large).map_or(false, |page_end| page_end <= end) {
            if count == out.len() {
                return Err(PlanError::BufferTooSmall);
            }
            let virt = virt_of(at).ok_or(PlanError::AddressOverflow)?;
            out[count] = Mapping {
                virt: VirtAddr::new(virt),
                phys: PhysAddr::new(at),
                len: large,
                flags: PageFlags::present(),
            };
            count += 1;
            at += large;
        }
    }
    Ok(count)
}

/// 规划**恒等映射**（`virt == phys`），返回产出的映射条数。
pub fn plan_identity(
    ranges: &[UsableRange],
    out: &mut [Mapping],
    large: u64,
) -> Result<usize, PlanError> {
    plan_with(ranges, out, large, Some)
}

/// 规划 **HHDM** 映射（`virt == offset + phys`）。
pub fn plan_hhdm(
    ranges: &[UsableRange],
    offset: u64,
    out: &mut [Mapping],
    large: u64,
) -> Result<usize, PlanError> {
    plan_with(ranges, out, large, |phys| offset.checked_add(phys))
}

/// 规划**内核高区**映射：物理区间 [phys_base, phys_base+len) 映射到
/// [virt_base, virt_base+len)，保持 `virt = virt_base + (phys - phys_base)`。
///
/// 复用 `plan_with`：对齐与遍历逻辑单点定义；位移的减法下溢与加法溢出都报
/// `AddressOverflow`（不回绕、不静默丢弃）。
pub fn plan_kernel_high(
    phys_base: u64,
    virt_base: u64,
    len: u64,
    out: &mut [Mapping],
    large: u64,
) -> Result<usize, PlanError> {
    let span = [UsableRange { base: PhysAddr::new(phys_base), length: len }];
    plan_with(&span, out, large, |phys| {
        let delta = phys.checked_sub(phys_base)?;
        virt_base.checked_add(delta)
    })
}

#[cfg(test)]
mod tests {
    use std::format;
    use std::string::String;
    use std::vec::Vec;

    #[test]
    fn every_plan_error_has_a_distinct_human_readable_message() {
        let mut seen: Vec<String> = Vec::new();
        for err in [
            PlanError::InvalidPageSize,
            PlanError::BufferTooSmall,
            PlanError::AddressOverflow,
        ] {
            let text = format!("{err}");
            assert!(!text.is_empty(), "每条错误都必须有消息");
            assert!(!seen.contains(&text), "消息不得重复: {text}");
            seen.push(text);
        }
    }
    use super::{Mapping, PlanError, plan_identity};
    use crate::usable::UsableRange;
    use arch::addr::PhysAddr;

    const LARGE: u64 = 2 * 1024 * 1024;

    fn range(base: u64, length: u64) -> UsableRange {
        UsableRange { base: PhysAddr::new(base), length }
    }

    #[test]
    fn a_range_is_aligned_down_and_up_to_the_large_page_size() {
        // 0x10_0000 已对齐；长度 3 个大页 + 一点余量 → 只映射 3 个大页。
        let ranges = [range(0x10_0000, 3 * LARGE + 0x1000)];
        let mut out = [Mapping::EMPTY; 4];
        let count = plan_identity(&ranges, &mut out, LARGE).expect("规划成功");
        // 0x10_0000 未按 2 MiB 对齐：先向上对齐到 0x20_0000，故只剩 2 个大页。
        assert_eq!(count, 2);
        assert_eq!(out[0].virt.as_u64(), 0x20_0000);
        assert_eq!(out[0].phys.as_u64(), 0x20_0000);
        assert_eq!(out[0].len, LARGE);
        assert_eq!(out[1].virt.as_u64(), 0x20_0000 + LARGE);
        assert!(out[0].flags.is_present());
    }

    #[test]
    fn a_partial_head_is_dropped() {
        // 起点未对齐：先向上对齐，头部不足一页的部分不映射。
        let ranges = [range(0x10_0000 + 0x1000, 2 * LARGE)];
        let mut out = [Mapping::EMPTY; 4];
        let count = plan_identity(&ranges, &mut out, LARGE).expect("规划成功");
        assert_eq!(count, 1, "向上对齐后只剩一个大页");
        assert_eq!(out[0].phys.as_u64(), 0x20_0000, "对齐后应是 2 MiB 边界");
    }

    #[test]
    fn ranges_shorter_than_one_page_are_skipped() {
        // 两段都放不下一个完整的对齐大页：第一段长度不足，第二段基址未对齐。
        let ranges = [range(0x20_0000, LARGE - 1), range(0x30_0000, LARGE)];
        let mut out = [Mapping::EMPTY; 4];
        let count = plan_identity(&ranges, &mut out, LARGE).expect("规划成功");
        assert_eq!(count, 0, "两段都容不下一个完整的对齐大页");
        // 第三段基址对齐且长度足够：应产出 1 条。
        let aligned = [range(0x40_0000, LARGE)];
        let count2 = plan_identity(&aligned, &mut out, LARGE).expect("规划成功");
        assert_eq!(count2, 1);
        assert_eq!(out[0].phys.as_u64(), 0x40_0000);
    }

    #[test]
    fn a_too_small_output_buffer_is_reported() {
        let ranges = [range(0x20_0000, 3 * LARGE)];
        let mut out = [Mapping::EMPTY; 2];
        assert_eq!(plan_identity(&ranges, &mut out, LARGE), Err(PlanError::BufferTooSmall));
    }

    #[test]
    fn a_zero_page_size_is_rejected() {
        let ranges = [range(0x10_0000, LARGE)];
        let mut out = [Mapping::EMPTY; 4];
        assert_eq!(plan_identity(&ranges, &mut out, 0), Err(PlanError::InvalidPageSize));
    }

    #[test]
    fn mapping_count_never_overflows_the_range() {
        // 极大长度：规划必须报错或停止，不能无限循环。
        let ranges = [range(0x10_0000, u64::MAX - 0x10_0000)];
        let mut out = [Mapping::EMPTY; 2];
        assert_eq!(plan_identity(&ranges, &mut out, LARGE), Err(PlanError::BufferTooSmall));
    }
}

#[cfg(test)]
mod hhdm_tests {
    use super::{Mapping, PlanError, plan_hhdm};
    use crate::usable::UsableRange;
    use arch::addr::PhysAddr;

    const LARGE: u64 = 2 * 1024 * 1024;
    const OFFSET: u64 = 0xffff_8000_0000_0000;

    fn range(base: u64, length: u64) -> UsableRange {
        UsableRange { base: PhysAddr::new(base), length }
    }

    #[test]
    fn virtual_is_offset_plus_physical() {
        // 基址 0x20_0000 = 2 MiB，本身即 LARGE 的整数倍。
        // 长度 2 * LARGE：产出 2 页（0x20_0000 与 0x40_0000）。
        let ranges = [range(0x20_0000, 2 * LARGE)];
        let mut out = [Mapping::EMPTY; 4];
        let count = plan_hhdm(&ranges, OFFSET, &mut out, LARGE).expect("规划成功");
        assert_eq!(count, 2);
        assert_eq!(out[0].phys.as_u64(), 0x20_0000);
        assert_eq!(out[0].virt.as_u64(), OFFSET + 0x20_0000);
        assert_eq!(out[1].phys.as_u64(), 0x20_0000 + LARGE);
        assert_eq!(out[1].virt.as_u64(), OFFSET + 0x20_0000 + LARGE);
    }

    #[test]
    fn an_offset_addition_that_overflows_is_rejected() {
        // 物理基址极大 + 偏移极大：virt 计算必然溢出，必须报错而不是回绕。
        let ranges = [range(0xffff_ffff_0000_0000, LARGE)];
        let mut out = [Mapping::EMPTY; 4];
        assert_eq!(
            plan_hhdm(&ranges, OFFSET, &mut out, LARGE),
            Err(PlanError::AddressOverflow)
        );
    }

    #[test]
    fn a_too_small_output_buffer_is_reported() {
        let ranges = [range(0x20_0000, 3 * LARGE)];
        let mut out = [Mapping::EMPTY; 2];
        assert_eq!(plan_hhdm(&ranges, OFFSET, &mut out, LARGE), Err(PlanError::BufferTooSmall));
    }

    #[test]
    fn a_zero_page_size_is_rejected() {
        let ranges = [range(0x20_0000, LARGE)];
        let mut out = [Mapping::EMPTY; 4];
        assert_eq!(plan_hhdm(&ranges, OFFSET, &mut out, 0), Err(PlanError::InvalidPageSize));
    }
}

#[cfg(test)]
mod kernel_high_tests {
    use super::{Mapping, PlanError, plan_kernel_high};
    

    const LARGE: u64 = 2 * 1024 * 1024;
    const VIRT_BASE: u64 = 0xffff_ffff_8000_0000;

    #[test]
    fn a_two_page_span_maps_with_a_constant_delta() {
        // phys_base = 0x20_0000（2 MiB 对齐），virt_base 如上；两页。
        let mut out = [Mapping::EMPTY; 4];
        let count = plan_kernel_high(0x20_0000, VIRT_BASE, 2 * LARGE, &mut out, LARGE).expect("规划成功");
        assert_eq!(count, 2);
        assert_eq!(out[0].phys.as_u64(), 0x20_0000);
        assert_eq!(out[0].virt.as_u64(), VIRT_BASE);
        assert_eq!(out[1].phys.as_u64(), 0x20_0000 + LARGE);
        assert_eq!(out[1].virt.as_u64(), VIRT_BASE + LARGE);
    }

    #[test]
    fn an_unaligned_physical_base_is_aligned_up_and_virt_follows() {
        // phys_base 加 0x1000：向上对齐到 0x20_0000，virt 仍是 VIRT_BASE。
        let mut out = [Mapping::EMPTY; 4];
        let count = plan_kernel_high(0x20_0000 + 0x1000, VIRT_BASE, 2 * LARGE, &mut out, LARGE).expect("规划成功");
        assert_eq!(count, 1, "对齐后只剩一页");
        assert_eq!(out[0].phys.as_u64(), 0x40_0000);
        // 位移相对 phys_base：0x40_0000 - 0x20_1000 = 0x1F_F000
        assert_eq!(out[0].virt.as_u64(), VIRT_BASE + 0x1F_F000);
    }

    #[test]
    fn a_virtual_overflow_is_rejected() {
        let mut out = [Mapping::EMPTY; 4];
        assert_eq!(
            plan_kernel_high(0x20_0000, u64::MAX - 0x1000, 2 * LARGE, &mut out, LARGE),
            Err(PlanError::AddressOverflow)
        );
    }

    #[test]
    fn capacity_and_page_size_are_checked() {
        let mut out = [Mapping::EMPTY; 1];
        assert_eq!(
            plan_kernel_high(0x20_0000, VIRT_BASE, 2 * LARGE, &mut out, LARGE),
            Err(PlanError::BufferTooSmall)
        );
        let mut out2 = [Mapping::EMPTY; 4];
        assert_eq!(
            plan_kernel_high(0x20_0000, VIRT_BASE, LARGE, &mut out2, 0),
            Err(PlanError::InvalidPageSize)
        );
    }
}
