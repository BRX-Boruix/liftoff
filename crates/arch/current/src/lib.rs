//! 架构实现选择器：把具体实现接到抽象上（ADR-050）。
//!
//! 目标构建启用 `impl-x86_64`（默认），宿主测试启用 `impl-mock`；两者互斥，
//! 都未启用时编译失败。

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(all(feature = "impl-x86_64", feature = "impl-mock"))]
compile_error!("`impl-x86_64` 与 `impl-mock` 互斥，只能启用一个");

#[cfg(not(any(feature = "impl-x86_64", feature = "impl-mock")))]
compile_error!("必须启用一个实现 feature：`impl-x86_64` 或 `impl-mock`");

#[cfg(feature = "impl-x86_64")]
pub use x86_64::platform::X86_64 as PlatformImpl;

/// 当前架构的页表实现（选择器职责：把具体实现接到抽象上）。
#[cfg(feature = "impl-x86_64")]
pub use x86_64::paging::X86PageTable;

/// spinup trampoline（当前实现 = x86_64 的汇编 + 低地址缓冲封装）。
#[cfg(feature = "impl-x86_64")]
pub use x86_64::spinup;

/// CPU 特性探测（NX / LA57）。跳板参数必须来自探测而非硬编码假设（S04）。
#[cfg(feature = "impl-x86_64")]
pub use x86_64::features;

/// LAPIC 寄存器常量与 ICR 编码（S3 的纯逻辑部分）。
#[cfg(feature = "impl-x86_64")]
pub use x86_64::lapic;

/// AP 启动跳板的参数块、搬运与布局常量（S5–S7）。
#[cfg(feature = "impl-x86_64")]
pub use x86_64::ap;

#[cfg(feature = "impl-mock")]
pub mod mock {
    //! 宿主测试用实现：不触碰硬件，但状态机行为真实（不是"假数据"）。

    use arch::addr::{PAGE_SIZE, PhysAddr, VirtAddr};
    use arch::paging::{MapError, PageFlags, PageTable};
    use arch::platform::{InterruptState, Platform};
    use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

    /// mock 页表的**映射条数上限** —— **有界** ✓，超出如实报 `OutOfMemory` ✗（不静默丢映射）。
    pub const MOCK_PAGE_TABLE_CAPACITY: usize = 32;

    /// 一条宿主侧映射。
    ///
    /// 以**整段**为单位存（而不是逐页）✓：容量才有意义，`translate` 靠页内偏移算 ✓。
    #[derive(Clone, Copy)]
    struct Entry {
        virt: u64,
        phys: u64,
        len: u64,
        /// **2 MiB 大页**（用于"拒绝拆分"的判断 ✓）。
        large: bool,
        flags: PageFlags,
    }

    /// 宿主测试用页表：**不碰真实内存** ✓，但语义按 `PageTable` 的契约 ✓。
    ///
    /// **能力范围如实声明** ✓：只支持 **4 KiB** 粒度 ✗；`map_range`（2 MiB）返回
    /// `UnsupportedGranularity`，**不假装支持** ✗。
    /// 只支持**整段**操作 —— 只覆盖一段的一部分时返回 `UnsupportedGranularity`，
    /// **不静默拆分** ✗（与真实现同一约定 ✓）。
    pub struct MockPageTable {
        entries: [Option<Entry>; MOCK_PAGE_TABLE_CAPACITY],
    }

    impl MockPageTable {
        /// 空页表（仅宿主测试使用）。
        pub fn new() -> Self {
            Self { entries: [None; MOCK_PAGE_TABLE_CAPACITY] }
        }

        fn validate(virt: VirtAddr, phys: PhysAddr, len: u64) -> Result<(), MapError> {
            if len == 0 {
                return Err(MapError::Empty);
            }
            if virt.as_u64() % PAGE_SIZE != 0 {
                return Err(MapError::MisalignedVirt);
            }
            if phys.as_u64() % PAGE_SIZE != 0 {
                return Err(MapError::MisalignedPhys);
            }
            if len % PAGE_SIZE != 0 {
                return Err(MapError::MisalignedLength);
            }
            virt.as_u64().checked_add(len).ok_or(MapError::Overflow)?;
            phys.as_u64().checked_add(len).ok_or(MapError::Overflow)?;
            Ok(())
        }

        fn index_of(&self, v: u64) -> Option<usize> {
            self.entries.iter().position(|e| match e {
                Some(entry) => v >= entry.virt && v < entry.virt + entry.len,
                None => false,
            })
        }
    }

    impl Default for MockPageTable {
        fn default() -> Self {
            Self::new()
        }
    }

    impl PageTable for MockPageTable {
        fn map_range(&mut self, _virt: VirtAddr, _phys: PhysAddr, _len: u64, _flags: PageFlags) -> Result<(), MapError> {
            // **如实拒绝** ✓：mock 只有 4 KiB 粒度，不假装支持 2 MiB ✗。
            Err(MapError::UnsupportedGranularity)
        }

        fn map_range_pages(&mut self, virt: VirtAddr, phys: PhysAddr, len: u64, flags: PageFlags) -> Result<(), MapError> {
            Self::validate(virt, phys, len)?;
            let slot = self.entries.iter().position(|e| e.is_none()).ok_or(MapError::OutOfMemory)?;
            self.entries[slot] = Some(Entry {
                virt: virt.as_u64(),
                phys: phys.as_u64(),
                len,
                large: false,
                flags,
            });
            Ok(())
        }

        fn translate(&self, virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
            let v = virt.as_u64();
            let entry = self.index_of(v).map(|i| self.entries[i])?;
            let entry = entry?;
            Some((PhysAddr::new(entry.phys + (v - entry.virt)), entry.flags))
        }

        fn unmap(&mut self, virt: VirtAddr, len: u64) -> Result<(), MapError> {
            Self::validate(virt, PhysAddr::new(0), len)?;
            let (start, end) = (virt.as_u64(), virt.as_u64() + len);
            for entry in self.entries.iter_mut() {
                let Some(e) = *entry else { continue };
                let (es, ee) = (e.virt, e.virt + e.len);
                if ee <= start || es >= end {
                    continue;
                }
                // **只允许整段解除** ✓ —— 部分覆盖会改变别的地址的粒度 ✗。
                if es < start || ee > end {
                    return Err(MapError::UnsupportedGranularity);
                }
                *entry = None;
            }
            // **未映射不算错** ✓：解除的目标状态就是"不存在"（与真实现同一约定 ✓）。
            Ok(())
        }

        fn protect(&mut self, virt: VirtAddr, len: u64, flags: PageFlags) -> Result<(), MapError> {
            Self::validate(virt, PhysAddr::new(0), len)?;
            let (start, end) = (virt.as_u64(), virt.as_u64() + len);
            let mut touched = false;
            for entry in self.entries.iter_mut() {
                let Some(mut e) = *entry else { continue };
                let (es, ee) = (e.virt, e.virt + e.len);
                if ee <= start || es >= end {
                    continue;
                }
                if es != start || ee != end {
                    // **拒绝而不是静默拆分** ✓（与真实现同一约定 ✓）。
                    return Err(MapError::UnsupportedGranularity);
                }
                e.flags = flags;
                *entry = Some(e);
                touched = true;
            }
            if touched {
                Ok(())
            } else {
                // **未映射必须报错** ✗ —— 假装成功会让调用方以为"权限已设" ✓。
                Err(MapError::Unmapped)
            }
        }

        unsafe fn activate(&self) {
            // **如实的不动作** ✓：宿主上没有可激活的页表 ✗ —— 不是假装激活成功 ✓。
        }
    }

    /// 输出记录缓冲的容量。
    ///
    /// **有界** ✓：溢出必须**可观测**（见 `output_overflowed`）✗ —— 静默丢弃会让"输出被截断"
    /// 看起来像"输出就这么多" ✗，而宿主测试正是靠这些字节做断言 ✓。
    pub const OUTPUT_CAPACITY: usize = 512;

    /// 已记录的输出字节。
    static OUTPUT: [AtomicU8; OUTPUT_CAPACITY] = [const { AtomicU8::new(0) }; OUTPUT_CAPACITY];
    /// 已记录的字节数（**不超过容量** ✓）。
    static OUTPUT_LEN: AtomicUsize = AtomicUsize::new(0);
    /// 是否发生过溢出（**如实记录，不隐藏** ✓）。
    static OUTPUT_OVERFLOW: AtomicBool = AtomicBool::new(false);

    /// 宿主测试用的中断开关状态。
    ///
    /// 并发说明：单测串行访问该原子量；不使用锁，避免测试间互相阻塞。
    static INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(true);

    /// 宿主测试用平台。
    pub struct Mock;

    /// 是否已初始化。
    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    impl Mock {
        /// 重置初始化标志（仅宿主测试使用）。
        pub fn reset_init_flag() {
            INITIALIZED.store(false, Ordering::SeqCst);
        }

        /// 是否已初始化（仅宿主测试使用）。
        pub fn initialized() -> bool {
            INITIALIZED.load(Ordering::SeqCst)
        }
        /// 显式设置中断开关状态（仅宿主测试使用）。
        pub fn set_interrupts_enabled(enabled: bool) {
            INTERRUPTS_ENABLED.store(enabled, Ordering::SeqCst);
        }

        /// 当前中断开关状态（仅宿主测试使用）。
        pub fn interrupts_enabled() -> bool {
            INTERRUPTS_ENABLED.load(Ordering::SeqCst)
        }

        /// 清空输出记录（含溢出标记）✓。
        pub fn reset_output() {
            OUTPUT_LEN.store(0, Ordering::SeqCst);
            OUTPUT_OVERFLOW.store(false, Ordering::SeqCst);
        }

        /// 把已记录的输出拷进 `out`，返回**真实长度** ✓。
        ///
        /// **返回真实长度而不是容量** ✓ —— 调用方据此知道"到底写了多少"，不会被静默填零误导 ✗。
        pub fn output(out: &mut [u8]) -> usize {
            let len = OUTPUT_LEN.load(Ordering::SeqCst).min(out.len());
            for (index, slot) in out[..len].iter_mut().enumerate() {
                *slot = OUTPUT[index].load(Ordering::SeqCst);
            }
            len
        }

        /// 记录是否**曾经溢出**（即输出被截断过）✓。
        pub fn output_overflowed() -> bool {
            OUTPUT_OVERFLOW.load(Ordering::SeqCst)
        }
    }

    impl Platform for Mock {
        fn init() {
            INITIALIZED.store(true, Ordering::SeqCst);
        }

        fn name() -> &'static str {
            "mock"
        }

        unsafe fn jump_to(_entry: u64) -> ! {
        // 测试替身不应被调用：万一有测试走到跳转，就响亮失败，而不是静默通过。
        panic!("测试替身不应被调用")
    }

    fn halt() -> ! {
            loop {
                core::hint::spin_loop();
            }
        }

        fn write_byte(byte: u8) {
            // **记录真实字节** ✓ —— 这是"零伪数据"的正面形态：既不编造输出，也不静默丢弃 ✓。
            let index = OUTPUT_LEN.load(Ordering::SeqCst);
            if index < OUTPUT_CAPACITY {
                OUTPUT[index].store(byte, Ordering::SeqCst);
                OUTPUT_LEN.store(index + 1, Ordering::SeqCst);
            } else {
                // 溢出**如实标记** ✓ —— 不假装"输出就这么多" ✗。
                OUTPUT_OVERFLOW.store(true, Ordering::SeqCst);
            }
        }

        fn bsp_lapic_id() -> u32 {
            // 宿主没有 APIC；**这一层由真机覆盖**（与读 CR4 同样的边界处理）。
            // 返回 0 是「无此信息」的如实表达，不是伪造一个看起来合理的 ID。
            0
        }

        fn disable_interrupts() -> InterruptState {
            InterruptState::from_enabled(INTERRUPTS_ENABLED.swap(false, Ordering::SeqCst))
        }

        fn restore_interrupts(state: InterruptState) {
            INTERRUPTS_ENABLED.store(state.was_enabled(), Ordering::SeqCst);
        }
    }
}

#[cfg(feature = "impl-mock")]
pub use mock::Mock as PlatformImpl;

#[cfg(all(test, feature = "impl-mock"))]
mod tests {
    use crate::mock::{Mock, MockPageTable, OUTPUT_CAPACITY};
    use arch::addr::{PAGE_SIZE, PhysAddr, VirtAddr};
    use arch::paging::{MapError, PageFlags, PageTable};
    use arch::platform::Platform;

    fn mapped(v: u64) -> (MockPageTable, PageFlags) {
        let mut pt = MockPageTable::new();
        let flags = PageFlags::present().with(PageFlags::writable());
        pt.map_range_pages(VirtAddr::new(v), PhysAddr::new(v + 0x1000_0000), PAGE_SIZE, flags)
            .expect("映射应成功");
        (pt, flags)
    }

    #[test]
    fn mock_page_table_translates_with_the_page_offset_and_the_flags() {
        let (pt, flags) = mapped(0x4000_0000);
        let (phys, got) = pt.translate(VirtAddr::new(0x4000_0123)).expect("应命中");
        assert_eq!(phys.as_u64(), 0x5000_0123, "物理地址要带页内偏移");
        assert_eq!(got, flags, "权限要原样返回");
        assert!(pt.translate(VirtAddr::new(0x4000_1000)).is_none(), "未映射就是未映射");
    }

    #[test]
    fn mock_page_table_reports_its_own_failure_paths() {
        let mut pt = MockPageTable::new();
        let p = PhysAddr::new(0x1000);
        assert_eq!(pt.map_range_pages(VirtAddr::new(0), p, 0, PageFlags::present()), Err(MapError::Empty));
        assert_eq!(pt.map_range_pages(VirtAddr::new(1), p, PAGE_SIZE, PageFlags::present()), Err(MapError::MisalignedVirt));
        assert_eq!(pt.map_range_pages(VirtAddr::new(0), PhysAddr::new(1), PAGE_SIZE, PageFlags::present()), Err(MapError::MisalignedPhys));
        assert_eq!(pt.map_range_pages(VirtAddr::new(0), p, PAGE_SIZE + 1, PageFlags::present()), Err(MapError::MisalignedLength));
        // 2 MiB 粒度**如实拒绝** ✓ —— mock 只支持 4 KiB，不假装支持 ✗。
        assert_eq!(pt.map_range(VirtAddr::new(0), p, PAGE_SIZE, PageFlags::present()), Err(MapError::UnsupportedGranularity));
    }

    #[test]
    fn mock_page_table_refuses_to_split_and_reports_unmapped_protect() {
        let (mut pt, _) = mapped(0x4000_0000);
        // 未映射改权限**必须报错** ✗，不能假装成功 ✓。
        assert_eq!(pt.protect(VirtAddr::new(0x8000_0000), PAGE_SIZE, PageFlags::present()), Err(MapError::Unmapped));
        // **整段覆盖**是允许的 ✓ —— `mapped()` 建的 run 恰好一页，所以这一条**应当成功** ✗。
        // （我第一版把它写成"必须被拒绝" ✗ —— 那是我把"整段"和"部分"搞混了：
        //  一页的 run 被整段保护，本来就是合法的整段操作 ✓。）
        pt.protect(VirtAddr::new(0x4000_0000), PAGE_SIZE, PageFlags::present())
            .expect("整段覆盖应当允许");
        // **部分覆盖**才必须拒绝 ✓ —— 建一个**两页**的 run，只保护其中一页。
        let mut two = MockPageTable::new();
        two.map_range_pages(
            VirtAddr::new(0x6000_0000),
            PhysAddr::new(0x7000_0000),
            PAGE_SIZE * 2,
            PageFlags::present(),
        )
        .expect("两页映射应成功");
        assert_eq!(
            two.protect(VirtAddr::new(0x6000_0000), PAGE_SIZE, PageFlags::present()),
            Err(MapError::UnsupportedGranularity),
            "只覆盖两页 run 的一部分时**不得静默拆分** ✗"
        );
        // 整段覆盖两页则允许 ✓。
        two.protect(VirtAddr::new(0x6000_0000), PAGE_SIZE * 2, PageFlags::present())
            .expect("整段两页应当允许");
    }

    #[test]
    fn mock_records_the_bytes_it_is_given() {
        // **零伪数据**的正面形态：不编造输出，也不静默丢弃 ✓。
        Mock::reset_output();
        for byte in b"abc" {
            Mock::write_byte(*byte);
        }
        let mut buf = [0u8; 8];
        let len = Mock::output(&mut buf);
        assert_eq!(len, 3, "必须返回**真实长度**，不是容量");
        assert_eq!(&buf[..3], b"abc");
        assert!(!Mock::output_overflowed());
    }

    #[test]
    fn mock_output_recording_is_bounded_and_says_so() {
        Mock::reset_output();
        for _ in 0..(OUTPUT_CAPACITY + 10) {
            Mock::write_byte(b'x');
        }
        let mut buf = std::vec![0u8; OUTPUT_CAPACITY];
        assert_eq!(Mock::output(&mut buf), OUTPUT_CAPACITY, "记录长度必须**封顶**，不越界");
        assert!(Mock::output_overflowed(), "溢出必须**可观测**，不能静默丢弃 ✗");
    }

    #[test]
    fn reset_output_clears_both_the_bytes_and_the_overflow_flag() {
        Mock::reset_output();
        for _ in 0..(OUTPUT_CAPACITY + 1) {
            Mock::write_byte(b'y');
        }
        assert!(Mock::output_overflowed());
        Mock::reset_output();
        assert!(!Mock::output_overflowed(), "重置必须连溢出标记一起清");
        let mut buf = [0u8; 4];
        assert_eq!(Mock::output(&mut buf), 0);
    }


    #[test]
    fn mock_init_is_observable_and_idempotent() {
        Mock::reset_init_flag();
        assert!(!Mock::initialized());
        Mock::init();
        assert!(Mock::initialized());
        Mock::init();
        assert!(Mock::initialized(), "重复初始化不应回退状态");
    }

    #[test]
    fn mock_reports_its_own_name() {
        assert_eq!(<Mock as Platform>::name(), "mock");
    }

    #[test]
    fn mock_tracks_the_interrupt_state_machine() {
        Mock::set_interrupts_enabled(true);
        let first = Mock::disable_interrupts();
        assert!(first.was_enabled(), "首次关中断应报告此前为开");
        let second = Mock::disable_interrupts();
        assert!(!second.was_enabled(), "已关中断时应报告此前为关");
        Mock::restore_interrupts(first);
        assert!(Mock::interrupts_enabled(), "恢复后应回到开启");
        Mock::restore_interrupts(second);
        assert!(!Mock::interrupts_enabled(), "恢复为关状态应生效");
    }
}