//! 平台基础操作：引导器其余部分只经本 trait 接触平台。
//!
//! 只收引导器当前真正需要的能力（严格模式与 ADR-007：不预留空接口）：停机、
//! 诊断输出、中断开关。上下文切换、定时器等能力在真正需要时再收进来。

/// 中断开关状态。
///
/// 由实现经 [`InterruptState::from_enabled`] 构造，调用方只读取，避免 `bool` 语义
/// 在调用点失真（`restore_interrupts(true)` 读不出"恢复为开"还是"此前是开"）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InterruptState(bool);

impl InterruptState {
    /// 由实现构造：`enabled` 表示调用前中断是否已开启。
    #[inline]
    pub const fn from_enabled(enabled: bool) -> Self {
        Self(enabled)
    }

    /// 调用前中断是否已开启。
    #[inline]
    pub const fn was_enabled(self) -> bool {
        self.0
    }
}

/// 平台基础操作。
pub trait Platform {
    /// 初始化平台（诊断通道等）。必须在任何输出之前调用一次。
    fn init();

    /// 平台名（诊断用）。
    fn name() -> &'static str;

    /// 停机，不返回。
    fn halt() -> !;

    /// 输出一个字节（引导阶段的诊断通道）。
    fn write_byte(byte: u8);

    /// 关中断并返回此前状态。
    fn disable_interrupts() -> InterruptState;

    /// 恢复由 [`Platform::disable_interrupts`] 返回的状态。
    fn restore_interrupts(state: InterruptState);
}

#[cfg(test)]
mod tests {
    use super::InterruptState;

    #[test]
    fn interrupt_state_round_trips_the_previous_state() {
        assert!(InterruptState::from_enabled(true).was_enabled());
        assert!(!InterruptState::from_enabled(false).was_enabled());
    }

    #[test]
    fn interrupt_state_is_a_value_type() {
        let a = InterruptState::from_enabled(true);
        let b = a;
        assert_eq!(a, b);
        assert_ne!(a, InterruptState::from_enabled(false));
    }
}
