//! 落地：把映射规划交给**注入的**页表实现。
//!
//! 边界：本模块只认 `arch::paging::PageTable` trait；具体实现（x86_64 的 4 级页表）由
//! 入口/选择器注入（ADR-050）。
//!
//! 失败语义：任一条映射失败**立即上报**，且**不再尝试后续映射**（fail-fast）——
//! 半落地的页表是危险状态，调用方必须整体重试或中止，而不是当作部分成功继续。

use crate::plan::Mapping;
use arch::paging::{MapError, PageTable};

/// 落地结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApplyReport {
    /// 成功落地的映射条数。
    pub applied: usize,
}

/// 依次落地每条映射；任一条失败立即返回该错误。
pub fn apply<P: PageTable>(page_table: &mut P, plan: &[Mapping]) -> Result<ApplyReport, MapError> {
    for mapping in plan {
        page_table.map_range(mapping.virt, mapping.phys, mapping.len, mapping.flags)?;
    }
    Ok(ApplyReport { applied: plan.len() })
}

#[cfg(test)]
mod tests {
    use super::{ApplyReport, apply};
    use crate::plan::Mapping;
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::{MapError, PageFlags, PageTable};
    use std::vec::Vec;

    struct Fake {
        calls: Vec<(u64, u64, u64)>, 
        fail_at: Option<usize>,
    }

    impl PageTable for Fake {
        fn map_range(
            &mut self,
            virt: VirtAddr,
            phys: PhysAddr,
            len: u64,
            _flags: PageFlags,
        ) -> Result<(), MapError> {
            if Some(self.calls.len()) == self.fail_at {
                return Err(MapError::OutOfMemory);
            }
            self.calls.push((virt.as_u64(), phys.as_u64(), len));
            Ok(())
        }

        unsafe fn activate(&self) {}
    }

    fn mapping(virt: u64, phys: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(phys),
            len,
            flags: PageFlags::present(),
        }
    }

    #[test]
    fn an_empty_plan_touches_nothing() {
        let mut table = Fake { calls: Vec::new(), fail_at: None };
        let report = apply(&mut table, &[]).expect("空计划应当成功");
        assert_eq!(report.applied, 0);
        assert!(table.calls.is_empty());
    }

    #[test]
    fn mappings_are_forwarded_in_order() {
        let mut table = Fake { calls: Vec::new(), fail_at: None };
        let plan = [mapping(0x1000, 0x1000, 0x1000), mapping(0x2000, 0x2000, 0x1000)];
        let report = apply(&mut table, &plan).expect("落地成功");
        assert_eq!(report.applied, 2);
        assert_eq!(table.calls, std::vec![(0x1000, 0x1000, 0x1000), (0x2000, 0x2000, 0x1000)]);
    }

    #[test]
    fn a_failure_stops_the_rest() {
        let mut table = Fake { calls: Vec::new(), fail_at: Some(1) };
        let plan = [mapping(0x1000, 0x1000, 0x1000), mapping(0x2000, 0x2000, 0x1000), mapping(0x3000, 0x3000, 0x1000)];
        assert_eq!(apply(&mut table, &plan), Err(MapError::OutOfMemory));
        assert_eq!(table.calls.len(), 1, "失败后不得继续落地后续映射");
    }

    #[test]
    fn the_report_counts_only_fully_applied_mappings() {
        let mut table = Fake { calls: Vec::new(), fail_at: None };
        let plan = [mapping(0x1000, 0x1000, 0x1000)];
        let report: ApplyReport = apply(&mut table, &plan).expect("落地成功");
        assert_eq!(report.applied, plan.len());
    }
}