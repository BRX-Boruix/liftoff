//! Liftoff —— BORUIX 的 UEFI 引导程序。
//!
//! M2a：固件文件协议链读 ESP 测试文件（基线回归）。
//! M2b：枚举块设备 → CD001 探测 → 挂载 ISO9660 → 读内核路径，报长度与字节和。
//! 里程碑路线见 wiki/contributor/liftoff.md。

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod boruix;
mod config;
mod efi;
mod elf;
mod ext2;
mod handover;
mod iso9660;
mod paging;
mod serial;
mod smp;

use core::panic::PanicInfo;

/// M2a 读缓冲：单次 Read 的容量（只读小测试文件）。
const READ_BUF: usize = 512;

/// M2b 读缓冲上限：内核 ELF 上限 16MiB（当前内核 <1MiB，余量 16 倍）。
const ISO_BUF: usize = 16 * 1024 * 1024;

/// M2c 读缓冲：EXT2 直块寻址上限 12KiB（1024B × 12 直块）。
/// 超过即需要一级间接链支持——内核超过 12KiB 时是 M3 的扩展点。
const EXT_BUF: usize = 12 * 1024;

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
    // M4：EBS 需要句柄，入口单点登记（此后只读）。
    unsafe { efi::set_image_handle(image_handle) };
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

    // M5：RSDP 经配置表传递（UEFI spec 4.6.2 ACPI_20_GUID）。固件源指针只在
    // 引导服务期可靠（源页类型 AcpiReclaim 会被内核回收），立即拷贝到
    // LoaderData 永久区；m4_handover 填 RsdpResponse.address（HHDM 虚地址）。
    match efi::find_rsdp(st) {
        Some(src) => match copy_rsdp(bs, src) {
            Ok(dst) => RSDP_COPY.store(dst as usize, core::sync::atomic::Ordering::Release),
            Err(s) => m4_fail("rsdp copy", s),
        },
        None => {
            // 非致命：内核 acpi::init 对 NULL RSDP 退化（M4 行为）。
            serial::write(format_args!("[liftoff] RSDP not found in config table\n"));
        }
    }
    serial::write(format_args!(
        "[liftoff] contract: kernel={} base_rev={}\n",
        config::KERNEL_PATH, config::LIMINE_BASE_REVISION,
    ));

    m2a(image_handle, bs);
    m2b(bs);
    m2c(bs);

    serial::write(format_args!("[liftoff] M2c done\n"));
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

                // M3: 装载内核 ELF（读到什么装什么——验收链路对内容形状闭环）。
                match elf::load(blob.as_slice(), bs) {
                    Ok(lr) => {
                        report_load(&lr);
                        // M4: 全量交接（BORUIX v1）。
                        // 1) ELF 文件原始内容拷到 LoaderData 永久区（File.base 语义；
                        //    PoolBuf 是 BootServicesData，EBS 后不可依赖）。
                        match copy_persistent(bs, blob.as_slice()) {
                            Ok(file_base) => {
                                m4_handover(bs, &lr, file_base, n as u64);
                            }
                            Err(s) => m4_fail("persist blob", s),
                        }
                    }
                    Err(s) => {
                        let mut l = Str64::new();
                        let _ = l.push_str("M3: reject status=0x");
                        push_byte_hex(&mut l, s as u8);
                        contract(&l);
                    }
                }
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

// ================================ M4
// 全量交接：响应区 → 扫描填充 → 页表 → GOP → 内存映射 → EBS → 跳 kmain。


/// RSDP 拷贝区物理地址（efi_main 写入，m4_handover 读取；单遍引导无并发）。
static RSDP_COPY: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// 从固件 RSDP 拷贝到 EfiLoaderData 永久区（长度按 ACPI 规范 offset20 的
/// length 字段，最小 36；ACPI 1.0 源 20 字节）。返回拷贝区物理地址。
/// 两阶段读取与内核 arch-x86_64/acpi.rs 同款（S31/S19）。
fn copy_rsdp(bs: &efi::BootServices, src: *const u8) -> Result<*mut u8, usize> {
    let rev = unsafe { *src.add(15) };
    let len = if rev >= 2 {
        let l = unsafe { core::ptr::read_unaligned(src.add(20) as *const u32) } as usize;
        l.max(36)
    } else {
        20
    };
    let pages = (len + 0xFFF) / 0x1000;
    let mut addr: u64 = 0;
    let status = unsafe {
        (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, pages, &mut addr)
    };
    if efi::is_error(status) {
        return Err(0xC2);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src, addr as *mut u8, len);
    }
    Ok(addr as *mut u8)
}
/// 把 ELF blob 拷进 EfiLoaderData 永久区（连续单块，File.base 语义）。
fn copy_persistent(bs: &efi::BootServices, blob: &[u8]) -> Result<*mut u8, usize> {
    let pages = (blob.len() + 0xFFF) / 0x1000;
    let mut addr: u64 = 0;
    let status = unsafe {
        (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, pages, &mut addr)
    };
    if efi::is_error(status) {
        return Err(0xC0);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(blob.as_ptr(), addr as *mut u8, blob.len());
    }
    Ok(addr as *mut u8)
}

/// M4 契约行（m4 前缀区别于 M3）。
fn m4_fail(what: &str, code: usize) -> ! {
    let mut l = Str64::new();
    let _ = l.push_str("M4: fail ");
    let _ = l.push_str(what);
    let _ = l.push_str(" 0x");
    push_byte_hex(&mut l, code as u8);
    contract(&l);
    // M4 失败即交接失败，无退路：停机（串口已记录）。
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// 全量交接主链。调用后不返回（成功跳内核；失败停机）。
fn m4_handover(bs: &efi::BootServices, lr: &elf::LoadResult, file_base: *mut u8, file_len: u64) -> ! {
    serial::write(format_args!("[m4] handover begin\n"));

    // 0) 内核几何：物理落位 + 高链接 vbase（来自 ELF vbase）。
    let kphys = lr.image_base;
    let kvbase = lr.vbase;
    let ksize = (lr.image_size + 0x1F_FFFF) & !0x1F_FFFF; // 2MB 对齐向上

    // 1) RAM 上界：装内核时只分配了一块；完整 RAM 图要等 GetMemoryMap。
    //    页表覆盖用保守上界：先取一次内存映射（EBS 前可重取），取 CONVENTIONAL
    //    最大 physical_start+pages*4096。
    let mut probe = [0u8; efi::MEMMAP_BUF_SIZE];
    let (probe_map, probe_key, probe_desc_size) = match get_memory_map(bs, &mut probe) {
        Ok(x) => x,
        Err(s) => m4_fail("GetMemoryMap probe", s),
    };
    let _ = probe_key;
    let mut ram_top: u64 = 0;
    for i in 0..probe_map.len() / probe_desc_size {
        let d = unsafe { &*(probe_map.as_ptr().add(i * probe_desc_size) as *const efi::MemoryDescriptor) };
        if d.mem_type == efi::MEM_EFI_CONVENTIONAL {
            let top = d.physical_start + d.number_of_pages * 4096;
            if top > ram_top {
                ram_top = top;
            }
        }
    }
    // 2MB 对齐向上（页表大页粒度）
    ram_top = (ram_top + 0x1F_FFFF) & !0x1F_FFFF;
    if ram_top == 0 {
        m4_fail("no conventional ram", 0xC1);
    }
    serial::write(format_args!("[m4] ram_top={:#x}\n", ram_top));

    // 3) GOP 帧缓冲（非致命：内核 framebuffer response 为 NULL 时走退化）。
    let fb = efi::locate_protocol::<efi::GraphicsOutput>(bs, &efi::GOP_GUID).ok();
    let fb_mode_ref: Option<&efi::GraphicsOutputMode> = fb
        .filter(|p| !p.is_null())
        .and_then(|p| unsafe { (&*p).mode.as_ref() });
    // 帧缓冲物理上界（QEMU 的 fb BAR 在 RAM 顶端之外，HHDM 映射必须覆盖）。
    let mut fb_phys: u64 = 0;
    let mut fb_end: u64 = ram_top;
    if let Some(mode) = fb_mode_ref {
        let fb_base = mode.frame_buffer_base;
        fb_phys = fb_base;
        fb_end = ((fb_base + 16 * 1024 * 1024) + 0x1F_FFFF) & !0x1F_FFFF;
    }

    // 2) 页表（恒等 + LAPIC + HHDM(含 fb) + 内核高区）。必须知道 fb 端界后再建。
    let tables = match paging::build(bs, ram_top, fb_end, kphys, ksize, kvbase) {
        Ok(t) => t,
        Err(s) => m4_fail("paging", s),
    };
    serial::write(format_args!("[m4] pml4={:#x}\n", tables.pml4_phys));
    let mut handover_data = match fb_mode_ref {
        Some(mode) => {
            let info = unsafe { &*mode.info };
            let fbstruct = boruix::Framebuffer {
                // Limine 语义：address 是 HHDM 虚地址（内核直接解引用）。
                address: (mode.frame_buffer_base + boruix::HHDM_OFFSET) as *mut u8,
                width: info.horizontal_resolution as u64,
                height: info.vertical_resolution as u64,
                pitch: (info.pixels_per_scan_line * 4) as u64,
                bpp: 32,
                memory_model: 1, // RGB
                red_mask_size: 8,
                red_mask_shift: if info.pixel_format == efi::PIXEL_BGRX { 16 } else { 0 },
                green_mask_size: 8,
                green_mask_shift: 8,
                blue_mask_size: 8,
                blue_mask_shift: if info.pixel_format == efi::PIXEL_BGRX { 0 } else { 16 },
                reserved: [0; 7],
                edid_size: 0,
                edid: core::ptr::null_mut(),
            };
            Some(fbstruct)
        }
        None => None,
    };

    // 4) RSDP：efi_main 已从配置表找到并拷贝（RSDP_COPY）。Limine 语义：
    //    RsdpResponse.address 是内核可解引用的虚地址 → 填 HHDM 虚地址。
    //    拷贝区类型 EfiLoaderData → memmap 转换后标 BootloaderReclaimable，
    //    内核 pmm 不会回收，RSDP 内容在快照前稳定。
    let rsdp_phys = RSDP_COPY.load(core::sync::atomic::Ordering::Acquire) as u64;
    let rsdp_ptr: *mut u8 = if rsdp_phys != 0 {
        (rsdp_phys + boruix::HHDM_OFFSET) as *mut u8
    } else {
        core::ptr::null_mut()
    };


    // 5b) SMP page: 8 SmpInfo (32B each) + pointer array, single page;
    //     BootloaderReclaimable semantics keep it out of the kernel free list.
    let mut smp_page: u64 = 0;
    unsafe {
        let st_smp = (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, 1, &mut smp_page);
        if efi::is_error(st_smp) { m4_fail("smp page", st_smp); }
        core::slice::from_raw_parts_mut(smp_page as *mut u8, 4096).fill(0);
    }

    // 5) 响应区（LoaderData 单块）。
    let kernel_len = file_len;
    let mut hd = boruix::Handover::build(
        crate::boruix::HHDM_OFFSET,
        core::ptr::null_mut(), // memmap entries 由 EBS 前最后一张图填充
        0,
        handover_data.take().unwrap_or_else(|| boruix::Framebuffer {
            address: core::ptr::null_mut(), width: 0, height: 0, pitch: 0, bpp: 0,
            memory_model: 0, red_mask_size: 0, red_mask_shift: 0, green_mask_size: 0,
            green_mask_shift: 0, blue_mask_size: 0, blue_mask_shift: 0, reserved: [0; 7],
            edid_size: 0, edid: core::ptr::null_mut(),
        }),
        rsdp_ptr,
        kphys, kvbase, kernel_len,
        fb_phys,
    );
    // 5c) SMP 区指针（SmpInfo 在页首，指针数组在页 +256）。
    hd.smp_infos = smp_page as *mut boruix::SmpInfo;
    hd.smp_ptrs = (smp_page + 256) as *mut *mut boruix::SmpInfo;
    hd.file_struct.base = (file_base as u64 + boruix::HHDM_OFFSET) as *mut u8;

    // 6) 最终内存映射 + 填充 + 扫描 + EBS + 跳转（EBS 后无串口日志）。
    unsafe {
        handover::final_ebs_and_jump(bs, &mut hd, tables.pml4_phys, handover_entry_phys(lr), rsdp_phys);
    }
}

/// entry 物理地址 = entry 虚地址 - vbase + phys_base。
fn handover_entry_phys(lr: &elf::LoadResult) -> u64 {
    lr.entry - lr.vbase + lr.image_base
}

/// 取内存映射（单次，缓冲预分配）。返回 (切片, map_key, desc_size)。
fn get_memory_map<'a>(
    bs: &efi::BootServices,
    buf: &'a mut [u8],
) -> Result<(&'a [u8], usize, usize), usize> {
    let mut size: usize = buf.len();
    let mut key: usize = 0;
    let mut dsize: usize = 0;
    let mut dver: u32 = 0;
    let status = unsafe {
        (bs.get_memory_map)(&mut size, buf.as_mut_ptr(), &mut key, &mut dsize, &mut dver)
    };
    if efi::is_error(status) {
        return Err(0xC2);
    }
    Ok((&buf[..size], key, dsize))
}

/// M3 装载报告：把 LoadResult 逐行打成串口契约行（与 elf_oracle.py 同源）。
fn report_load(lr: &elf::LoadResult) {
    for i in 0..lr.segs {
        let r = &lr.reports[i];
        let mut l = Str64::new();
        let _ = l.push_str("M3: seg");
        push_dec(&mut l, i);
        let _ = l.push_str(" rel=0x");
        push_rel_hex(&mut l, r.rel);
        let _ = l.push_str(" size=");
        push_dec(&mut l, r.memsz as usize);
        let _ = l.push_str(" sum16=0x");
        push_sum16(&mut l, r.sum16 as u32);
        contract(&l);
    }
    let mut l = Str64::new();
    let _ = l.push_str("M3: segs=");
    push_dec(&mut l, lr.segs);
    let _ = l.push_str(" entry=0x");
    push_rel_hex(&mut l, lr.entry);
    contract(&l);
    let mut l = Str64::new();
    let _ = l.push_str("M3: vbase=0x");
    push_rel_hex(&mut l, lr.vbase);
    let _ = l.push_str(" image_size=");
    push_dec(&mut l, lr.image_size as usize);
    contract(&l);
    // base 是运行期观测值（物理地址固件决定），oracle 无法预算，不作断言；
    // 它同时是 M4 协议交接的锚点数据。
    let mut l = Str64::new();
    let _ = l.push_str("M3: base=0x");
    push_rel_hex(&mut l, lr.image_base);
    contract(&l);
    let mut l = Str64::new();
    let _ = l.push_str("M3: total_mem=");
    push_dec(&mut l, lr.total_mem as usize);
    let _ = l.push_str(" total_sum16=0x");
    push_sum16(&mut l, lr.total_sum16 as u32);
    contract(&l);
    let mut l = Str64::new();
    let _ = l.push_str("M3: total_sum16=0x");
    push_sum16(&mut l, lr.total_sum16 as u32);
    contract(&l);
}

/// 64 位值 hex（小写，无前导零省略——固定 16 位宽与 oracle 一致）。
fn push_rel_hex(s: &mut Str64, v: u64) {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut started = false;
    for shift in (0..64).step_by(4).rev() {
        let nib = ((v >> shift) & 0xF) as usize;
        if nib != 0 || started || shift == 0 {
            s.push(HEX[nib] as char);
            started = true;
        }
    }
}

// ================================ M2c
// EXT2 真实链：磁盘设备（512B 块、非只读、非分区）→ 挂载 → 读 /BOOT/KERNIMG.BIN。

fn m2c(bs: &efi::BootServices) {
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
        fatal("locate block devices (m2c)", status);
    }
    // SAFETY: buf_raw 即本次 no_handles 对应的池缓冲。
    let Some(handles) = (unsafe { efi::HandleBuffer::wrap(bs, buf_raw as *mut core::ffi::c_void, no_handles) }) else {
        fatal("handle buffer null (m2c)", efi::EFI_UNSUPPORTED);
    };

    for &h in handles.handles() {
        let bio_raw = match efi::protocol_of::<efi::BlockIo>(bs, h, &efi::BLOCK_IO_GUID) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // SAFETY: 协议指针由固件保证有效。
        let bio = unsafe { &*bio_raw };
        // SAFETY: media 指针同上。
        let media = unsafe { &*bio.media };
        if media.logical_partition || media.read_only {
            continue; // M2c 只挂整盘可写介质（安装盘口径）；光盘归 M2b
        }
        let mut dev = iso9660::UefiBlock::new(bs, bio);
        let vol = match ext2::Volume::mount(&mut dev) {
            Ok(v) => v,
            Err(s) => {
                // 非 EXT2 的块设备（vvfat ESP 等）静默跳过是正确语义：
                // ext-nosig 变体依赖这条路径输出探测失败行。
                let mut l = Str64::new();
                let _ = l.push_str("M2C: mount failed status=0x");
                push_byte_hex(&mut l, s as u8);
                contract(&l);
                continue;
            }
        };
        let ok = Str64::from("M2C: mount ok");
        contract(&ok);

        match vol.open_path(&mut dev, config::EXT_KERNEL_PATH) {
            Ok(f) => {
                let mut blob = [0u8; EXT_BUF];
                let n = match vol.read_file(&mut dev, &f, &mut blob) {
                    Ok(n) => n,
                    Err(s) => fatal("ext read", s),
                };
                let mut sum: u32 = 0;
                for &b in &blob[..n] {
                    sum = sum.wrapping_add(b as u32);
                }
                let mut l = Str64::new();
                let _ = l.push_str("M2C: len=");
                push_dec(&mut l, n);
                contract(&l);
                let mut l = Str64::new();
                let _ = l.push_str("M2C: sum=0x");
                push_sum16(&mut l, sum);
                contract(&l);
            }
            Err(efi::EFI_NOT_FOUND) => {
                let mut line = Str64::new();
                let _ = line.push_str("M2C: open failed status=0x");
                push_status_hex(&mut line, efi::EFI_NOT_FOUND);
                contract(&line);
            }
            Err(s) => {
                let mut l = Str64::new();
                let _ = l.push_str("M2C: open failed status=0x");
                push_byte_hex(&mut l, s as u8);
                contract(&l);
            }
        }
        return; // 命中第一块 EXT2 盘即完成 M2c 验收
    }
    let fail = Str64::from("M2C: no ext2 disk");
    contract(&fail);
}

/// 单字节 hex（内部错误码契约行）。
fn push_byte_hex(s: &mut Str64, v: u8) {
    const HEX: &[u8] = b"0123456789abcdef";
    s.push(HEX[(v >> 4) as usize] as char);
    s.push(HEX[(v & 0xF) as usize] as char);
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