//! Liftoff —— BORUIX 的 UEFI 引导程序。
//!
//! M2a：固件文件协议链读 ESP 测试文件（基线回归）。
//! M2b：枚举块设备 → CD001 探测 → 挂载 ISO9660 → 读内核路径，报长度与字节和。
//! 里程碑路线见 wiki/contributor/liftoff.md。

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod config;
mod efi;
mod iso9660;
mod serial;

use core::panic::PanicInfo;

/// M2a 读缓冲：单次 Read 的容量（只读小测试文件）。
const READ_BUF: usize = 512;

/// M2b 读缓冲上限：内核 ELF 上限 16MiB（当前内核 <1MiB，余量 16 倍）。
const ISO_BUF: usize = 16 * 1024 * 1024;

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

/// 契约行（验收脚本断言）与普通日志的前缀区分。
fn contract(line: &Str64) {
    serial::write(format_args!("{line}\n"));
}

#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(
    image_handle: efi::Handle,
    system_table: *mut core::ffi::c_void,
) -> usize {
    serial::init();
    serial::write(format_args!("[liftoff] M2b entry\n"));

    // 1. 系统表与引导服务：签名不符则只剩串口可用。
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

    m2a(image_handle, bs);
    m2b(bs);

    serial::write(format_args!("[liftoff] M2b done\n"));
    efi::EFI_SUCCESS
}

// ================================ M2a
// 固件 SFS 链基线：证明我们自己的协议调用代码正确（vvfat ESP 上）。

fn m2a(image_handle: efi::Handle, bs: &efi::BootServices) {
    let loaded_raw = match efi::protocol_of::<efi::LoadedImage>(bs, image_handle, &efi::LOADED_IMAGE_GUID) {
        Ok(p) => p,
        Err(s) => fatal("LoadedImage not on image handle", s),
    };
    // SAFETY: HandleProtocol 成功返回的接口由固件保证有效至 ExitBootServices。
    let loaded = unsafe { &*loaded_raw };
    let sfs_raw = match efi::protocol_of::<efi::SimpleFileSystem>(bs, loaded.device_handle, &efi::SIMPLE_FILE_SYSTEM_GUID) {
        Ok(p) => p,
        Err(s) => fatal("SimpleFileSystem not on device handle", s),
    };
    // SAFETY: 同上。
    let sfs = unsafe { &*sfs_raw };

    let mut root_raw: *mut efi::FileProtocol = core::ptr::null_mut();
    let status = unsafe { (sfs.open_volume)(sfs, &mut root_raw) };
    if efi::is_error(status) {
        fatal("open volume", status);
    }
    let root_guard = efi::FileGuard(root_raw);
    // SAFETY: open_volume 成功，root_raw 非空且有效。
    let root = unsafe { &*root_guard.0 };

    let mut name_buf = [0u16; 64];
    let Some(name) = efi::wide_nul(config::TEST_FILE, &mut name_buf) else {
        fatal("test file name too long", efi::EFI_INVALID_PARAMETER);
    };
    let mut file_raw: *mut efi::FileProtocol = core::ptr::null_mut();
    let status = unsafe {
        (root.open)(root, &mut file_raw, name.as_ptr(), efi::FILE_MODE_READ, 0)
    };
    if efi::is_error(status) {
        // 文件不存在是合法分支（missing 变体）：报告后仍经守卫关卷。
        let mut line = Str64::new();
        let _ = line.push_str("M2A: open failed status=0x");
        push_status_hex(&mut line, status);
        contract(&line);
        return;
    }
    let file_guard = efi::FileGuard(file_raw);
    // SAFETY: open 成功，file_raw 非空且有效。
    let file = unsafe { &*file_raw };

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
            break; // 空文件恰好一次 Read 即 0 字节
        }
        for &b in &buf[..want] {
            sum = sum.wrapping_add(b as u32);
        }
        total = total.checked_add(want).expect("file size overflow");
    }

    let mut l = Str64::new();
    let _ = l.push_str("M2A: len=");
    push_dec(&mut l, total);
    contract(&l);
    let mut l = Str64::new();
    let _ = l.push_str("M2A: sum=0x");
    push_sum16(&mut l, sum);
    contract(&l);
    drop(file_guard);
}

// ================================ M2b
// ISO9660 真实链：块设备枚举 → 光驱判定 → 挂载 → 读内核路径。

fn m2b(bs: &efi::BootServices) {
    // 枚举全部 BlockIo 句柄。
    let mut no_handles: usize = 0;
    let mut buf_raw: *mut efi::Handle = core::ptr::null_mut();
    let status = unsafe {
        (bs.locate_handle_buffer)(
            efi::SEARCH_BY_PROTOCOL,
            &efi::BLOCK_IO_GUID as *const efi::Guid as *const core::ffi::c_void,
            core::ptr::null_mut(),
            &mut no_handles,
            &mut buf_raw,
        )
    };
    if efi::is_error(status) {
        fatal("locate block devices", status);
    }
    // SAFETY: buf_raw 即本次 no_handles 对应的池缓冲。
    let Some(handles) = (unsafe { efi::HandleBuffer::wrap(bs, buf_raw as *mut core::ffi::c_void, no_handles) }) else {
        fatal("handle buffer null", efi::EFI_UNSUPPORTED);
    };
    serial::write(format_args!("[m2b] block io handles: {}\n", handles.handles().len()));

    for &h in handles.handles() {
        let bio_raw = match efi::protocol_of::<efi::BlockIo>(bs, h, &efi::BLOCK_IO_GUID) {
            Ok(p) => p,
            Err(_) => continue, // 竞争移除等情形：跳过
        };
        // SAFETY: 协议指针由固件保证有效。
        let bio = unsafe { &*bio_raw };
        // SAFETY: media 指针同上。
        let media = unsafe { &*bio.media };
        if media.logical_partition {
            continue; // 分区设备交给其父盘（M2c EXT2 再处理分区表）
        }
        serial::write(format_args!(
            "[m2b] handle {:#x}: block_size={} last_block={} ro={}\n",
            h as usize, media.block_size, media.last_block, media.read_only,
        ));

        // 光驱判定（与 brxLimine 同款：只读 + 2048 块即视为光学介质）。
        if !(media.read_only && media.block_size == iso9660::SECTOR as u32) {
            continue;
        }

        let mut dev = iso9660::UefiBlock::new(bs, bio);
        let vol = match iso9660::Volume::mount(bs, &mut dev) {
            Ok(v) => v,
            Err(s) => {
                serial::write(format_args!("[m2b] not iso9660 (err {s:#x})\n"));
                continue;
            }
        };
        let ok = Str64::from("M2B: mount ok");
        contract(&ok);


        match vol.open_path(bs, &mut dev, config::ISO_KERNEL_PATH) {
            Ok(f) => {
                let mut blob = match iso9660::PoolBuf::new(bs, ISO_BUF) {
                    Ok(b) => b,
                    Err(s) => fatal("iso read buffer", s),
                };
                let n = match vol.read_file(&mut dev, &f, blob.as_slice()) {
                    Ok(n) => n,
                    Err(s) => fatal("iso read", s),
                };
                let mut sum: u32 = 0;
                for &b in &blob.as_slice()[..n] {
                    sum = sum.wrapping_add(b as u32);
                }
                let mut l = Str64::new();
                let _ = l.push_str("M2B: len=");
                push_dec(&mut l, n);
                contract(&l);
                let mut l = Str64::new();
                let _ = l.push_str("M2B: sum=0x");
                push_sum16(&mut l, sum);
                contract(&l);
            }
            Err(efi::EFI_NOT_FOUND) => {
                let mut line = Str64::new();
                let _ = line.push_str("M2B: open failed status=0x");
                push_status_hex(&mut line, efi::EFI_NOT_FOUND);
                contract(&line);
            }
            Err(s) => fatal("iso open", s),
        }
        return; // 命中第一块可挂载光盘即完成 M2b 验收
    }
    // 未找到任何可挂载的 ISO 卷：0x11 是 iso9660 模块"探测失败"内部码。
    let fail = Str64::from("M2B: mount failed status=0x11");
    contract(&fail);
}

// ================================ 输出辅助（无 alloc）

/// 定长栈字符串。
pub struct Str64 {
    buf: [u8; 96],
    len: usize,
}

impl Str64 {
    pub fn from(s: &str) -> Self {
        let mut me = Str64::new();
        let _ = me.push_str(s);
        me
    }
}

impl core::fmt::Display for Str64 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // SAFETY: buf[..len] 只写入过 UTF-8 片段。
        let s = core::str::from_utf8(&self.buf[..self.len]).map_err(|_| core::fmt::Error)?;
        f.write_str(s)
    }
}

impl Str64 {
    pub fn new() -> Self {
        Str64 { buf: [0; 96], len: 0 }
    }

    pub fn push_str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.buf.len() {
            return; // 截断即缺陷：96 字节足够全部契约行，超出属 bug
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    pub fn push(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        let enc = c.encode_utf8(&mut tmp).as_bytes();
        if self.len + enc.len() > self.buf.len() {
            return;
        }
        self.buf[self.len..self.len + enc.len()].copy_from_slice(enc);
        self.len += enc.len();
    }
}

/// 十进制拼接。
fn push_dec(s: &mut Str64, mut v: usize) {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for &c in &tmp[i..] {
        s.push(c as char);
    }
}

/// 4 位 hex（sum16 契约行）。
fn push_sum16(s: &mut Str64, v: u32) {
    const HEX: &[u8] = b"0123456789abcdef";
    for shift in [12, 8, 4, 0] {
        s.push(HEX[((v >> shift) & 0xF) as usize] as char);
    }
}

/// 16 位 hex（UEFI 状态码契约行）。
fn push_status_hex(s: &mut Str64, v: usize) {
    const HEX: &[u8] = b"0123456789abcdef";
    for shift in [60, 56, 52, 48, 44, 40, 36, 32, 28, 24, 20, 16, 12, 8, 4, 0] {
        s.push(HEX[((v >> shift) & 0xF) as usize] as char);
    }
}