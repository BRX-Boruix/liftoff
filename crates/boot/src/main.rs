//! liftoff 可执行入口：极薄。
//!
//! 边界：本文件只做 `efi_main` 转发与失败处理；一切逻辑在 `boot` lib 里（可宿主测试）。

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use arch::platform::Platform;
use boot::PlatformImpl;
use core::panic::PanicInfo;
use firmware_current::current::SystemTable;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    PlatformImpl::halt()
}

/// UEFI 入口（`x86_64-unknown-uefi` 链接符号 `efi_main`）。
#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    image_handle: *mut core::ffi::c_void,
    system_table: *mut core::ffi::c_void,
) -> usize {
    PlatformImpl::init();
    // `image_handle` 必须传进去：退出引导服务时要用它（`ExitBootServices` 的第一个参数）。
    match boot::entry::start(system_table.cast::<SystemTable>(), image_handle) {
        Ok(_) => 0,
        Err(_) => {
            // 失败：输出一行失败诊断后停机 —— 不静默返回“成功”。
            for byte in b"[liftoff] entry failed\n" {
                PlatformImpl::write_byte(*byte);
            }
            PlatformImpl::halt()
        }
    }
}
