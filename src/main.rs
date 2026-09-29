//! Liftoff —— BORUIX 的 UEFI 引导程序。
//!
//! M2a：经固件文件协议读测试文件，报告长度与字节和。
//! 链路：LoadedImage → DeviceHandle → SimpleFileSystem → OpenVolume → Open → Read。
//! 后续里程碑见 wiki/contributor/liftoff.md。

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod config;
mod efi;
mod serial;

use core::panic::PanicInfo;

/// 读缓冲：单次 Read 的容量。
/// 当前固定 512：M2a 只读小测试文件；动态尺寸读取（内核 ELF 为 MB 级）
/// 在 M2b 改走 AllocatePool，届时删除此常量。
const READ_BUF: usize = 512;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::write(format_args!("[liftoff] PANIC: {info}\n"));
    halt();
}

/// 关中断后停机。引导失败没有恢复路径，停机是唯一诚实的终态。
fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// 失败即报告并停机。
fn fatal(msg: &str, status: usize) -> ! {
    serial::write(format_args!("[liftoff] FAIL: {msg} status=0x{status:016x}\n"));
    halt();
}

/// 报告文件打开失败。这是验收契约行（M2A: 前缀），missing 变体断言它；
/// 文件不存在是合法测试分支，不是引导失败。
fn report_open_failure(status: usize) {
    serial::write(format_args!("M2A: open failed status=0x{status:016x}\n"));
}

#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    image_handle: efi::Handle,
    system_table: *mut core::ffi::c_void,
) -> usize {
    serial::init();
    serial::write(format_args!("[liftoff] M2a entry\n"));

    // 1. 系统表：签名不符则只剩串口可用。
    let Some(st) = (unsafe { efi::SystemTable::from_ptr(system_table) }) else {
        serial::write(format_args!("[liftoff] system table signature mismatch\n"));
        halt();
    };
    let Some(bs) = (unsafe { st.boot_services.as_ref() }) else {
        fatal("boot services null", efi::EFI_UNSUPPORTED);
    };
    if bs.hdr.signature != efi::BOOT_SERVICES_SIGNATURE {
        fatal("boot services signature mismatch", efi::EFI_UNSUPPORTED);
    }
    serial::write(format_args!("[liftoff] boot services ok\n"));
    serial::write(format_args!(
        "[liftoff] contract: kernel={} base_rev={}\n",
        config::KERNEL_PATH, config::LIMINE_BASE_REVISION,
    ));

    // 2. LoadedImage：自身镜像的 DeviceHandle 即引导介质。
    let loaded_raw = match efi::protocol_of::<efi::LoadedImage>(bs, image_handle, &efi::LOADED_IMAGE_GUID) {
        Ok(p) => p,
        Err(s) => fatal("LoadedImage not on image handle", s),
    };
    // SAFETY: HandleProtocol 成功返回的接口由固件保证有效至 ExitBootServices。
    let loaded = unsafe { &*loaded_raw };
    serial::write(format_args!("[liftoff] device_handle={:#x}\n", loaded.device_handle as usize));

    // 3. 引导介质上的 SimpleFileSystem。
    let sfs_raw = match efi::protocol_of::<efi::SimpleFileSystem>(bs, loaded.device_handle, &efi::SIMPLE_FILE_SYSTEM_GUID) {
        Ok(p) => p,
        Err(s) => fatal("SimpleFileSystem not on device handle", s),
    };
    // SAFETY: 同上。
    let sfs = unsafe { &*sfs_raw };

    // 4. OpenVolume。root 由守卫持有：从此处起任何路径退出都会 Close（S18）。
    let mut root_raw: *mut efi::FileProtocol = core::ptr::null_mut();
    let status = unsafe { (sfs.open_volume)(sfs, &mut root_raw) };
    if efi::is_error(status) {
        fatal("open volume", status);
    }
    let root_guard = efi::FileGuard(root_raw);
    // SAFETY: open_volume 成功，root_raw 非空且有效。经由守卫字段引用，
    // 所有权归属一目了然（S18）。
    let root = unsafe { &*root_guard.0 };

    // 5. Open(TEST_FILE, READ)。
    let mut name_buf = [0u16; 64];
    let Some(name) = efi::wide_nul(config::TEST_FILE, &mut name_buf) else {
        fatal("test file name too long", efi::EFI_INVALID_PARAMETER);
    };
    let mut file_raw: *mut efi::FileProtocol = core::ptr::null_mut();
    let status = unsafe {
        (root.open)(root, &mut file_raw, name.as_ptr(), efi::FILE_MODE_READ, 0)
    };
    if efi::is_error(status) {
        // 文件不存在是合法分支：报告后仍走 root_guard 的 Drop 关卷，正常返回。
        report_open_failure(status);
        return efi::EFI_SUCCESS;
    }
    let file_guard = efi::FileGuard(file_raw);
    // SAFETY: open 成功，file_raw 非空且有效。
    let file = unsafe { &*file_raw };

    // 6. Read 循环：单次 Read 可能部分填充，读到 END_OF_FILE 为止。
    let mut buf = [0u8; READ_BUF];
    let mut total = 0usize;
    let mut sum: u32 = 0;
    loop {
        let mut want = buf.len();
        let status = unsafe { (file.read)(file, &mut want, buf.as_mut_ptr()) };
        if status == efi::EFI_END_OF_FILE {
            break;
        }
        if efi::is_error(status) {
            fatal("read", status);
        }
        if want == 0 {
            // 规范允许返回 SUCCESS 且 0 字节（空文件恰好一次 Read 即如此）。
            break;
        }
        for &b in &buf[..want] {
            sum = sum.wrapping_add(b as u32);
        }
        total = total.checked_add(want).expect("file size overflow");
    }

    // 7. 报告。boottest.ps1 断言这两行与预计算值一致。
    serial::write(format_args!("M2A: len={total}\n"));
    serial::write(format_args!("M2A: sum=0x{sum:04x}\n"));

    drop(file_guard); // 显式 Close 文件，随后隐式 drop root_guard 关卷
    serial::write(format_args!("[liftoff] M2a done\n"));
    efi::EFI_SUCCESS
}