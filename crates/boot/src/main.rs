#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! liftoff 可执行入口（骨架）：仅最小 UEFI 入口，尚无引导逻辑。

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// UEFI 入口（`x86_64-unknown-uefi` 目标链接符号 `efi_main`）。
#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    _image_handle: *mut core::ffi::c_void,
    _system_table: *mut core::ffi::c_void,
) -> usize {
    0
}
