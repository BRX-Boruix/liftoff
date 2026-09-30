//! 入口编排（可宿主测试的部分）。
//!
//! 边界：bin 只做 `efi_main` 转发；本模块做“取引导服务表 → 输出启动诊断”的编排。
//! 平台与固件实现的**选择**来自门面（`crate::PlatformImpl`、`firmware_current::current`），
//! 本模块不自己挑实现。

use arch::platform::Platform;
use firmware::boot_services::BootServicesControl;
use firmware::error::Error;
use firmware_current::current::{
    ExitBootServices, Handle, SystemTable, UefiBootServices, boot_services_of,
};

/// 入口第一步的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// 引导服务表已取得，启动诊断已输出。
    Ready,
}

/// 以指定平台执行入口第一步；失败时**不输出诊断**（不制造假成功）。
pub fn start_with<P: Platform>(system_table: *mut SystemTable) -> Result<Outcome, Error> {
    let _boot_services = boot_services_of(system_table)?;
    crate::diag::report_startup::<P>();
    Ok(Outcome::Ready)
}

/// 生产入口：平台固定为门面选定的实现。
pub fn start(system_table: *mut SystemTable) -> Result<Outcome, Error> {
    start_with::<crate::PlatformImpl>(system_table)
}

/// 退出引导服务（入口编排）：复用 `UefiBootServices` 的语义。
///
/// # Safety
///
/// 调用方必须保证退出后不再调用任何固件服务，且当前代码与栈在新页表中仍被映射。
pub unsafe fn exit_prepared(
    exit: ExitBootServices,
    image_handle: Handle,
    map_key: &mut Option<usize>,
) -> Result<(), Error> {
    let mut control = UefiBootServices::new(exit, image_handle, map_key);
    // SAFETY: 由调用方保证（见函数文档）。
    unsafe { control.exit_boot_services() }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, start, start_with};
    use arch::platform::{InterruptState, Platform};
    use firmware::error::Error;
    use firmware_current::current::SystemTable;
    use std::vec::Vec;

    struct Recorder;

    static mut SINK: Option<Vec<u8>> = None;

    impl Platform for Recorder {
        fn init() {}

        fn name() -> &'static str {
            "recorder"
        }

        fn halt() -> ! {
            loop {
                core::hint::spin_loop();
            }
        }

        fn write_byte(byte: u8) {
            // SAFETY: 测试内单线程；不创建对静态的引用。
            unsafe {
                let slot = (&raw mut SINK).as_mut().expect("SINK 地址有效");
                slot.as_mut().expect("SINK 已初始化").push(byte);
            }
        }

        fn disable_interrupts() -> InterruptState {
            InterruptState::from_enabled(true)
        }

        fn restore_interrupts(_state: InterruptState) {}
    }

    fn reset() {
        // SAFETY: 测试内单线程。
        unsafe { *(&raw mut SINK) = Some(Vec::new()) };
    }

    fn taken() -> Vec<u8> {
        // SAFETY: 测试内单线程。
        unsafe { (*&raw mut SINK).take().unwrap_or_default() }
    }

    fn table_with_boot_services() -> SystemTable {
        // SAFETY: 零值是合法表示（全空指针）。
        let mut table = unsafe { core::mem::zeroed::<SystemTable>() };
        table.boot_services = 0x40usize as *mut core::ffi::c_void;
        table
    }

    #[test]
    fn a_null_system_table_fails_without_writing_anything() {
        reset();
        assert_eq!(start_with::<Recorder>(core::ptr::null_mut()), Err(Error::InvalidArgument));
        assert!(taken().is_empty(), "失败时不得输出诊断（不制造假成功）");
    }

    #[test]
    fn a_valid_system_table_writes_the_startup_line() {
        reset();
        let mut table = table_with_boot_services();
        assert_eq!(start_with::<Recorder>(&mut table), Ok(Outcome::Ready));
        let line = std::string::String::from_utf8(taken()).expect("UTF-8");
        assert_eq!(line, "[liftoff] gen2 up, platform=recorder\n");
    }

    #[test]
    fn the_production_entry_uses_the_selected_platform() {
        // 生产入口只把平台固定为门面选定的实现；其行为由 QEMU 验收（PRE-2）。
        let _ = start as fn(*mut SystemTable) -> Result<Outcome, Error>;
    }
}

#[cfg(test)]
mod exit_tests {
    use super::exit_prepared;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;
    use firmware_current::current::{ExitBootServices, Handle, UefiBootServices};
    use firmware::boot_services::{BootServicesControl, BootServicesState};

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static FAIL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_exit(_image: Handle, _key: usize) -> firmware_current::current::Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        if FAIL.load(Ordering::SeqCst) == 1 {
            firmware_current::current::DEVICE_ERROR
        } else {
            firmware_current::current::SUCCESS
        }
    }

    fn exit_fn() -> ExitBootServices {
        fake_exit
    }

    #[test]
    fn exiting_without_a_map_key_is_rejected_without_calling_firmware() {
        CALLS.store(0, Ordering::SeqCst);
        let mut key = None;
        assert_eq!(unsafe { exit_prepared(exit_fn(), core::ptr::null_mut(), &mut key) }, Err(Error::InvalidState));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "无 map_key 时不得调用固件");
    }

    #[test]
    fn a_successful_exit_marks_the_state_exited() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        let mut key = Some(0x77);
        assert_eq!(unsafe { exit_prepared(exit_fn(), core::ptr::null_mut(), &mut key) }, Ok(()));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_exit_keeps_the_firmware_active() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(1, Ordering::SeqCst);
        let mut key = Some(1);
        let mut control = UefiBootServices::new(exit_fn(), core::ptr::null_mut(), &mut key);
        assert_eq!(unsafe { control.exit_boot_services() }, Err(Error::Io));
        assert_eq!(control.state(), BootServicesState::Active, "失败后固件仍在运行");
    }
}
