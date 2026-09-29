//! Liftoff —— BORUIX 的 UEFI 引导程序。
//!
//! 阶段一（M1）：入口签名校验 + 双通道输出。
//! 后续里程碑见 wiki/contributor/liftoff.md。

#![no_std]
#![no_main]

mod config;
mod efi;
mod serial;

use core::fmt::Write as _;
use core::panic::PanicInfo;

/// 宽字符缓冲容量：足够容纳最长的状态行。
const WIDE_BUF: usize = 256;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::write(format_args!("[liftoff] PANIC: {info}\n"));
    loop {
        core::hint::spin_loop();
    }
}

/// 经固件控制台输出。失败静默：串口才是观测主通道。
fn con_out(table: &efi::SystemTable, text: &str) {
    let con = table.con_out;
    if con.is_null() {
        return;
    }
    let mut buf = efi::alloc_vec::Vec::<u16, WIDE_BUF>::new();
    for c in text.encode_utf16() {
        buf.push(c);
    }
    buf.push(0);
    unsafe {
        let con_ref = &*con;
        (con_ref.output_string)(con_ref, buf.as_slice().as_ptr());
    }
}

#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    image_handle: *mut core::ffi::c_void,
    system_table: *mut core::ffi::c_void,
) -> usize {
    serial::init();

    serial::write(format_args!("[liftoff] M1 entry\n"));
    serial::write(format_args!(
        "[liftoff] image_handle={:#x} system_table={:#x}\n",
        image_handle as usize, system_table as usize
    ));

    // 系统表签名校验：不符则只能走串口报告并停机。
    let Some(table) = efi::SystemTable::from_ptr(system_table) else {
        serial::write(format_args!("[liftoff] system table signature mismatch, halting\n"));
        return efi::EFI_ERROR;
    };

    serial::write(format_args!("[liftoff] signature ok\n"));
    con_out(table, "Liftoff M1: console + serial alive\r\n");

    let rev = table.hdr.revision;
    serial::write(format_args!("[liftoff] firmware table revision {rev}\n"));

    efi::EFI_SUCCESS
}