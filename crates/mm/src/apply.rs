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
///
/// **粒度自动选择**：映射的 `virt`/`phys` 是否 2 MiB 对齐、`len` 是否为 2 MiB 倍数，
/// 决定走 `map_range`（2 MiB 大页）还是 `map_range_pages`（4 KiB 小页）。调用方
/// 不必关心粒度 —— 规划器产出的低 4 GiB 恒等映射（从 `0x1000` 起）会被自动路由到
/// 4 KiB 路径，而内核/HHDM/其余恒等区间照旧走大页。
///
/// 判据必须是**两个地址都满足大页对齐**且长度成倍：只看其一会在错位时把不合法的
/// 组合送进大页路径。
pub fn apply<P: PageTable>(page_table: &mut P, plan: &[Mapping]) -> Result<ApplyReport, MapError> {
    const LARGE: u64 = 2 * 1024 * 1024;
    for mapping in plan {
        let large_ok = mapping.virt.as_u64() % LARGE == 0
            && mapping.phys.as_u64() % LARGE == 0
            && mapping.len % LARGE == 0;
        if large_ok {
            page_table.map_range(mapping.virt, mapping.phys, mapping.len, mapping.flags)?;
        } else {
            page_table.map_range_pages(mapping.virt, mapping.phys, mapping.len, mapping.flags)?;
        }
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

    /// 两种粒度都记录进同一个 `calls`：粒度选择是 `apply` 的实现细节，
    /// 这些测试验证的是「转发顺序」与「fail-fast」，与走哪条路径无关。
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
        fn map_range_pages(
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

        // 这些假表不建模翻译：如实回答「未映射」，而不是假装知道。
        fn translate(&self, _virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
            None
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
    fn a_misaligned_mapping_goes_through_the_4kib_path() {
        // C1/DEBT-5：低 4 GiB 恒等映射从 0x1000 起（不满足 2 MiB 对齐），
        // `apply` 必须把它交给 4 KiB 路径而不是报 UnsupportedGranularity。
        struct Recording {
            large: Vec<(u64, u64, u64)>,
            pages: Vec<(u64, u64, u64)>,
        }
        impl PageTable for Recording {
            fn map_range(
                &mut self, virt: VirtAddr, phys: PhysAddr, len: u64, _: PageFlags,
            ) -> Result<(), MapError> {
                self.large.push((virt.as_u64(), phys.as_u64(), len));
                Ok(())
            }
            fn map_range_pages(
                &mut self, virt: VirtAddr, phys: PhysAddr, len: u64, _: PageFlags,
            ) -> Result<(), MapError> {
                self.pages.push((virt.as_u64(), phys.as_u64(), len));
                Ok(())
            }
            // 这些假表不建模翻译：如实回答「未映射」，而不是假装知道。
        fn translate(&self, _virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
            None
        }

        unsafe fn activate(&self) {}
        }

        let mut table = Recording { large: Vec::new(), pages: Vec::new() };
        let plan = [
            mapping(0x200000, 0x200000, 0x200000),  // 2 MiB 对齐 -> 大页
            mapping(0x1000, 0x1000, 0x200000),      // 只 4 KiB 对齐 -> 小页
        ];
        let report = apply(&mut table, &plan).expect("落地成功");
        assert_eq!(report.applied, 2);
        assert_eq!(table.large, std::vec![(0x200000, 0x200000, 0x200000)]);
        assert_eq!(table.pages, std::vec![(0x1000, 0x1000, 0x200000)]);
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