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

#[cfg(feature = "impl-mock")]
pub mod mock {
    //! 宿主测试用实现：不触碰硬件，但状态机行为真实（不是"假数据"）。

    use arch::platform::{InterruptState, Platform};
    use core::sync::atomic::{AtomicBool, Ordering};

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

        fn write_byte(_byte: u8) {
            // 宿主测试不产生输出；需要断言输出时再引入记录缓冲（届时另加测试）。
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
    use crate::mock::Mock;
    use arch::platform::Platform;


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