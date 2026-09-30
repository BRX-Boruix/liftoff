//! `firmware::boot_services::BootServicesControl` 的 UEFI 实现。
//!
//! 语义：`map_key` 必须来自最近一次成功的内存映射加载（否则固件会拒绝）；
//! **退出失败时固件仍在运行**，状态必须保持 `Active`（不能先标记已退出）。

use crate::boot_services_table::ExitBootServices;
use crate::status::status_to_error;
use crate::types::Handle;
use firmware::boot_services::{BootServicesControl, BootServicesState, ExitGuard};
use firmware::error::Error;

/// 基于 UEFI `ExitBootServices` 的退出控制。
pub struct UefiBootServices<'a> {
    exit_boot_services: ExitBootServices,
    image_handle: Handle,
    map_key: &'a mut Option<usize>,
    guard: ExitGuard,
}

impl<'a> UefiBootServices<'a> {
    /// 以 `ExitBootServices` 指针、映像句柄与共享的 `map_key` 构造。
    pub fn new(
        exit_boot_services: ExitBootServices,
        image_handle: Handle,
        map_key: &'a mut Option<usize>,
    ) -> Self {
        Self { exit_boot_services, image_handle, map_key, guard: ExitGuard::new() }
    }
}

impl BootServicesControl for UefiBootServices<'_> {
    fn state(&self) -> BootServicesState {
        self.guard.state()
    }

    unsafe fn exit_boot_services(&mut self) -> Result<(), Error> {
        if self.guard.state() == BootServicesState::Exited {
            return Err(Error::InvalidState);
        }
        let Some(key) = *self.map_key else {
            return Err(Error::InvalidState);
        };
        // SAFETY: 由调用方保证退出后不再调用任何固件服务，且当前代码与栈在切换后的页表中
        // 仍被映射（见 trait 的 SAFETY 契约）。
        let status = unsafe { (self.exit_boot_services)(self.image_handle, key) };
        if let Some(err) = status_to_error(status) {
            return Err(err);
        }
        self.guard.mark_exited()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UefiBootServices;
    use crate::types::{DEVICE_ERROR, Handle, Status, SUCCESS};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::boot_services::{BootServicesControl, BootServicesState};
    use firmware::error::Error;

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static SEEN_KEY: AtomicUsize = AtomicUsize::new(0);
    static FAIL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_exit(_image: Handle, map_key: usize) -> Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        SEEN_KEY.store(map_key, Ordering::SeqCst);
        if FAIL.load(Ordering::SeqCst) == 1 {
            DEVICE_ERROR
        } else {
            SUCCESS
        }
    }

    fn reset(fail: bool) {
        CALLS.store(0, Ordering::SeqCst);
        SEEN_KEY.store(0, Ordering::SeqCst);
        FAIL.store(if fail { 1 } else { 0 }, Ordering::SeqCst);
    }

    #[test]
    fn successful_exit_passes_the_key_and_marks_exited() {
        reset(false);
        let mut key = Some(0xABCD);
        let mut control = UefiBootServices::new(fake_exit, core::ptr::null_mut(), &mut key);
        assert_eq!(control.state(), BootServicesState::Active);
        unsafe { control.exit_boot_services() }.expect("退出成功");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(SEEN_KEY.load(Ordering::SeqCst), 0xABCD, "必须把 map_key 原样传回固件");
        assert_eq!(control.state(), BootServicesState::Exited);
    }

    #[test]
    fn failed_exit_keeps_the_firmware_active() {
        reset(true);
        let mut key = Some(1);
        let mut control = UefiBootServices::new(fake_exit, core::ptr::null_mut(), &mut key);
        assert_eq!(unsafe { control.exit_boot_services() }, Err(Error::Io));
        assert_eq!(control.state(), BootServicesState::Active, "失败后固件仍在运行，状态不得标记为已退出");
    }

    #[test]
    fn missing_key_and_repeat_exit_are_rejected_without_calling_firmware() {
        reset(false);
        let mut none_key = None;
        let mut control = UefiBootServices::new(fake_exit, core::ptr::null_mut(), &mut none_key);
        assert_eq!(unsafe { control.exit_boot_services() }, Err(Error::InvalidState));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "没有 map_key 时不得调用固件");

        reset(false);
        let mut key = Some(2);
        let mut control = UefiBootServices::new(fake_exit, core::ptr::null_mut(), &mut key);
        unsafe { control.exit_boot_services() }.expect("首次退出成功");
        assert_eq!(unsafe { control.exit_boot_services() }, Err(Error::InvalidState));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "重复退出不得再调用固件");
    }
}
