//! Liftoff gen2 -- BORUIX UEFI bootloader (clean-room rewrite).
//!
//! Skeleton: UEFI entry point, panic handler, serial console. Boot logic is
//! added stage by stage; each stage must build and be verified offline before
//! it is exercised under QEMU.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod efi;
mod serial;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::write(format_args!("[liftoff] PANIC: {info}\n"));
    halt()
}

/// Halt this CPU forever.
pub fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)); }
    }
}

/// UEFI entry point (the x86_64-unknown-uefi target links the symbol `efi_main`).
#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(_image: efi::Handle, _st: *mut efi::SystemTable) -> efi::Status {
    serial::init();
    serial::write(format_args!("[liftoff] gen2 skeleton up\n"));
    efi::SUCCESS
}
