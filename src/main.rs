//! Liftoff —— BORUIX 的 UEFI 引导程序。
//!
//! 职责：读取 EXT2 分区上的内核 ELF，按 Limine 协议填好请求响应，跳转内核入口。
//!
//! 当前为骨架：仅验证 UEFI 目标可编译并可被固件加载。

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod config;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // 骨架阶段：无控制台，直接停机。
    // TODO(M2): 接 UEFI Simple Text Output 输出 panic 信息与位置。
    loop {
        core::hint::spin_loop();
    }
}

/// UEFI 入口。
///
/// 固件以 Microsoft x64 调用约定调用此函数，
/// 参数为 EFI_HANDLE image_handle 与 EFI_SYSTEM_TABLE 指针。
///
/// TODO(M1): 接收并校验两个参数，取得 BootServices / RuntimeServices。
#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    _image_handle: *mut core::ffi::c_void,
    _system_table: *mut core::ffi::c_void,
) -> usize {
    // TODO(M1): 打开 Simple File System / Block I/O 协议。
    // TODO(M2): 挂载 EXT2，读取 config::KERNEL_PATH。
    // TODO(M3): 解析 ELF，映射段。
    // TODO(M4): 构造 Limine 请求响应，退出引导服务，跳转内核。
    config::EFI_SUCCESS
}
