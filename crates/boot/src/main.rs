//! liftoff 可执行入口：装配点。
//!
//! 本 crate 是唯一把架构实现（经 `current` 选择器）接到抽象上的地方（ADR-050）。
//! 当前只有最小启动诊断；引导逻辑按 ADR-049 的顺序逐步落地。

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use arch::platform::Platform;
use current::PlatformImpl;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    PlatformImpl::halt()
}

/// 输出一行启动诊断（引导阶段的唯一可观测通道）。
fn report_startup() {
    for byte in b"[liftoff] gen2 up, platform=" {
        PlatformImpl::write_byte(*byte);
    }
    for byte in PlatformImpl::name().bytes() {
        PlatformImpl::write_byte(byte);
    }
    PlatformImpl::write_byte(b'\n');
}

/// UEFI 入口（`x86_64-unknown-uefi` 目标链接符号 `efi_main`）。
#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    _image_handle: *mut core::ffi::c_void,
    _system_table: *mut core::ffi::c_void,
) -> usize {
    PlatformImpl::init();
    report_startup();
    0
}
