//! 页表接管的前置检查。
//!
//! 为什么需要：`PageTable::activate` 的 SAFETY 契约要求“切换后当前正在执行的代码与栈
//! 仍被映射”。本模块把**能检查的部分**显式化：给定“必须保持映射的区间”与规划结果，
//! 逐页检查是否都被覆盖。
//!
//! **不能检查的部分（如实写明）**：固件在切换后的行为、TLB/缓存一致性、以及当前
//! RIP/RSP 的实际取值（需汇编读取，由入口提供并作为 `must_stay` 传入）。

use crate::plan::Mapping;

/// 必须保持映射的地址区间。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MustStay {
    /// 起始虚拟地址。
    pub start: u64,
    /// 长度（字节）。
    pub len: u64,
}

/// 检查失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TakeoverError {
    /// 某个必须保持映射的地址没有被任何映射覆盖。
    Uncovered {
        /// 未被覆盖的地址。
        address: u64,
    },
    /// 区间长度为零（调用方错误）。
    EmptyRange,
}

/// 逐页检查 `must_stay` 的每个区间是否都被 `plan` 覆盖。
///
/// 页粒度取 `plan` 中第一条映射的长度（同一份规划里粒度一致）；`plan` 为空时
/// 任何非空 `must_stay` 都会报 `Uncovered`。
pub fn check_coverage(plan: &[Mapping], must_stay: &[MustStay]) -> Result<(), TakeoverError> {
    let large = plan.first().map(|m| m.len).unwrap_or(0);
    for span in must_stay {
        if span.len == 0 {
            return Err(TakeoverError::EmptyRange);
        }
        let mut at = span.start;
        let end = span.start.checked_add(span.len).ok_or(TakeoverError::Uncovered { address: span.start })?;
        while at < end {
            let covered = plan.iter().any(|m| {
                m.virt.as_u64() <= at && at < m.virt.as_u64().saturating_add(m.len)
            });
            if !covered {
                return Err(TakeoverError::Uncovered { address: at });
            }
            let step = if large == 0 { 1 } else { large };
            at = match at.checked_add(step) {
                Some(next) => next,
                None => break,
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MustStay, TakeoverError, check_coverage};
    use crate::plan::Mapping;
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::PageFlags;

    const LARGE: u64 = 2 * 1024 * 1024;

    fn mapping(virt: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(virt),
            len,
            flags: PageFlags::present(),
        }
    }

    #[test]
    fn a_fully_covered_span_passes() {
        let plan = [mapping(0x20_0000, LARGE), mapping(0x40_0000, LARGE)];
        let spans = [MustStay { start: 0x20_0000, len: 2 * LARGE }];
        assert_eq!(check_coverage(&plan, &spans), Ok(()));
    }

    #[test]
    fn a_gap_is_reported_with_its_address() {
        let plan = [mapping(0x20_0000, LARGE)];
        let spans = [MustStay { start: 0x20_0000, len: 2 * LARGE }];
        assert_eq!(
            check_coverage(&plan, &spans),
            Err(TakeoverError::Uncovered { address: 0x40_0000 })
        );
    }

    #[test]
    fn an_empty_plan_covers_nothing() {
        let spans = [MustStay { start: 0x20_0000, len: LARGE }];
        assert_eq!(
            check_coverage(&[], &spans),
            Err(TakeoverError::Uncovered { address: 0x20_0000 })
        );
    }

    #[test]
    fn a_zero_length_span_is_a_caller_error() {
        let plan = [mapping(0x20_0000, LARGE)];
        let spans = [MustStay { start: 0x20_0000, len: 0 }];
        assert_eq!(check_coverage(&plan, &spans), Err(TakeoverError::EmptyRange));
    }

    #[test]
    fn an_overflowing_span_is_reported_as_uncovered() {
        let plan = [mapping(0x20_0000, LARGE)];
        let spans = [MustStay { start: u64::MAX - 0x10, len: 0x100 }];
        assert_eq!(
            check_coverage(&plan, &spans),
            Err(TakeoverError::Uncovered { address: u64::MAX - 0x10 })
        );
    }
}
