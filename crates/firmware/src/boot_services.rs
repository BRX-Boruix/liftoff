//! 退出引导服务：固件运行期与引导器运行期的分界。
//!
//! 语义契约（Limine 交接要求 + ADR-051）：
//! - 退出成功后**不得**再调用任何固件服务（内存分配、文件、块设备、图形）；
//! - 调用方必须先取得所有仍需的数据（内存映射等），并保证当前代码与栈在新页表中仍被映射；
//! - 重复退出返回 `Error::InvalidState`（不静默成功）。

use crate::error::Error;

/// 引导服务状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootServicesState {
    /// 引导服务仍在运行。
    Active,
    /// 已退出引导服务（此后不得再调用固件服务）。
    Exited,
}

/// 退出引导服务的状态机；各实现共用（单点定义）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExitGuard {
    state: BootServicesState,
}

impl ExitGuard {
    /// 初始状态：引导服务运行中。
    pub const fn new() -> Self {
        Self { state: BootServicesState::Active }
    }

    /// 当前状态。
    pub const fn state(&self) -> BootServicesState {
        self.state
    }

    /// 标记为已退出；重复调用返回 `Error::InvalidState`。
    pub fn mark_exited(&mut self) -> Result<(), Error> {
        if self.state == BootServicesState::Exited {
            return Err(Error::InvalidState);
        }
        self.state = BootServicesState::Exited;
        Ok(())
    }
}

impl Default for ExitGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// 退出引导服务：固件层能力 trait 之一。
pub trait BootServicesControl {
    /// 当前状态。
    fn state(&self) -> BootServicesState;

    /// 退出引导服务；已退出时返回 `Error::InvalidState`。
    ///
    /// # Safety
    ///
    /// 调用方必须保证退出后不再调用任何固件服务，且当前正在执行的代码与栈在切换后的
    /// 页表中仍被映射（否则退出后立即故障）。
    unsafe fn exit_boot_services(&mut self) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::{BootServicesState, ExitGuard};
    use crate::error::Error;

    #[test]
    fn guard_starts_active() {
        let guard = ExitGuard::new();
        assert_eq!(guard.state(), BootServicesState::Active);
    }

    #[test]
    fn guard_transitions_to_exited_once() {
        let mut guard = ExitGuard::new();
        assert_eq!(guard.mark_exited(), Ok(()));
        assert_eq!(guard.state(), BootServicesState::Exited);
    }

    #[test]
    fn repeated_exit_is_rejected() {
        let mut guard = ExitGuard::new();
        guard.mark_exited().expect("首次退出应成功");
        assert_eq!(guard.mark_exited(), Err(Error::InvalidState));
        assert_eq!(guard.state(), BootServicesState::Exited);
    }

    #[test]
    fn state_is_a_value_type() {
        assert_eq!(BootServicesState::Active, BootServicesState::Active);
        assert_ne!(BootServicesState::Active, BootServicesState::Exited);
    }
}
