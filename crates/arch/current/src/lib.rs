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

    use arch::platform::{InterruptState, Platform};
    use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

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
    use crate::mock::{Mock, OUTPUT_CAPACITY};
    use arch::platform::Platform;

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