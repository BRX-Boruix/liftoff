//! ExitBootServices + 内存映射转换 + 内核跳转（M4）。
//!
//! EBS 流程（§7.4）：GetMemoryMap 取 map → 记 map_key → ExitBootServices(key)；
//! 失败（EFI_INVALID_PARAMETER，映射已变）重取重试，上限 EBS_MAX_RETRIES。
//! EBS 之后只允许直接硬件 IO（串口 0x3F8）。
//!
//! 内存映射转换（UEFI → Limine 语义）：
//!   LOADER_CODE / LOADER_DATA → BootloaderReclaimable（loader 自占区——响应
//!   区、内核文件拷贝、页表、RSDP 拷贝都从这类分配；标 usable 会被内核 pmm
//!   回收后踩踏。Limine 语义：bootloader reclaimable 在内核快照页表根之后
//!   才可回收，我们的内核 pmm 只把 Usable 计入 free list，安全）
//!   CONVENTIONAL → Usable
//!   BOOT_SERVICES_CODE/DATA → Usable（EBS 后固件服务内存释放，规范语义）
//!   ACPI_RECLAIM → AcpiReclaimable；ACPI_NVS → AcpiNvs
//!   RUNTIME_* / MMIO / 其他 → Reserved
//!   内核映像区单列 KernelAndModules（从 usable 中切出）
//!   framebuffer 区单列 Framebuffer
//!
//! 相邻同类型项合并（Limine 保证 usable/bootloader-reclaimable 不重叠且对齐，
//! 我们的输出同样保证）。

use crate::boruix;
use crate::efi;

// ---------------------------------------------------------------- 内存映射

/// UEFI 类型 → Limine 类型映射（模块头对照表）。
fn uefi_to_limine(uefi_type: u32) -> u64 {
    match uefi_type {
        efi::MEM_EFI_CONVENTIONAL
        | efi::MEM_EFI_BOOT_SERVICES_CODE
        | efi::MEM_EFI_BOOT_SERVICES_DATA => boruix::MEMMAP_USABLE,
        efi::MEM_EFI_LOADER_CODE | efi::MEM_EFI_LOADER_DATA => {
            boruix::MEMMAP_BOOTLOADER_RECLAIMABLE
        }
        efi::MEM_EFI_ACPI_RECLAIM => boruix::MEMMAP_ACPI_RECLAIMABLE,
        efi::MEM_EFI_ACPI_NVS => boruix::MEMMAP_ACPI_NVS,
        _ => boruix::MEMMAP_RESERVED,
    }
}

/// 转换 UEFI 描述符数组 → Limine MemmapEntry 数组（排序+合并）。
/// 输出写入 out_entries（容量 out_cap），返回实际项数。
pub fn convert_memmap(
    uefi_map: &[efi::MemoryDescriptor],
    out_entries: *mut boruix::MemmapEntry,
    out_cap: usize,
    kernel_base: u64,
    kernel_end: u64,
    fb_base: u64,
    fb_size: u64,
) -> Result<usize, usize> {
    // 1) 平铺转换（把内核/帧缓冲区从 usable 里剥出来）
    let mut tmp: [boruix::MemmapEntry; 128] = core::array::from_fn(|_| boruix::MemmapEntry { base: 0, len: 0, typ: 0 });
    let mut n = 0usize;
    let push = |tmp: &mut [boruix::MemmapEntry; 128], n: &mut usize, base: u64, len: u64, typ: u64| -> Result<(), usize> {
        if len == 0 {
            return Ok(());
        }
        if *n >= tmp.len() {
            return Err(0xB0); // 项数溢出
        }
        tmp[*n] = boruix::MemmapEntry { base, len, typ };
        *n += 1;
        Ok(())
    };
    for d in uefi_map {
        let base = d.physical_start;
        let len = d.number_of_pages * 4096;
        let t = uefi_to_limine(d.mem_type);
        if t == boruix::MEMMAP_USABLE {
            // 剥离内核与帧缓冲（都在 usable 区间内：UEFI 已为我们分配）
            let k_lo = kernel_base.max(base);
            let k_hi = kernel_end.min(base + len);
            let f_lo = fb_base.max(base);
            let f_hi = (fb_base + fb_size).min(base + len);
            // [base, base+len) 切成 ≤3 段：前段、内核/_fb、尾段
            // 先内核后帧缓冲（区间不重叠：帧缓冲 MMIO 通常不在 conventional；
            // 若重叠，属于固件异常，内核区间优先）。
            if base < k_lo {
                // clamp 到 desc 区间内：k_lo 可能在 base+len 之外（desc 不含
                // 内核时不截断会产出越界长度、usable 重叠——M5 实锤）。
                let hi = k_lo.min(base + len);
                push(&mut tmp, &mut n, base, hi - base, boruix::MEMMAP_USABLE)?;
            }
            if k_hi > k_lo {
                push(&mut tmp, &mut n, k_lo, k_hi - k_lo, boruix::MEMMAP_KERNEL_AND_MODULES)?;
            }
            let seg_start = k_hi.max(base);
            if seg_start < f_lo {
                // f_lo 也可能越 desc 上界（同上）。
                let hi = f_lo.min(base + len);
                push(&mut tmp, &mut n, seg_start, hi - seg_start, boruix::MEMMAP_USABLE)?;
            }
            if f_hi > f_lo {
                push(&mut tmp, &mut n, f_lo, f_hi - f_lo, boruix::MEMMAP_FRAMEBUFFER)?;
            }
            let tail = f_hi.max(seg_start);
            if tail < base + len {
                push(&mut tmp, &mut n, tail, base + len - tail, boruix::MEMMAP_USABLE)?;
            }
        } else {
            push(&mut tmp, &mut n, base, len, t)?;
        }
    }
    // 2) 按 base 排序（简单插入排序：项数小）
    for i in 1..n {
        let key = tmp[i];
        let mut j = i;
        while j > 0 && tmp[j - 1].base > key.base {
            tmp[j] = tmp[j - 1];
            j -= 1;
        }
        tmp[j] = key;
    }
    // 3) 合并相邻同类型
    let mut m = 0usize;
    for i in 0..n {
        if m > 0 && tmp[m - 1].typ == tmp[i].typ && tmp[m - 1].base + tmp[m - 1].len == tmp[i].base {
            tmp[m - 1].len += tmp[i].len;
        } else {
            tmp[m] = tmp[i];
            m += 1;
        }
    }
    if m > out_cap {
        return Err(0xB1);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), out_entries, m);
    }
    Ok(m)
}

// ---------------------------------------------------------------- 跳转

/// 跳转内核（裸汇编）：cli + cld → CR3 = pml4 → 近跳 entry（恒等映射）。
/// 长模式与 CS 沿用 UEFI（base=0 的 64 位 code 段）；内核 kmain 首句自切栈、
/// 自建 GDT。far ret 在 LLVM Intel 语法下编码不可靠（实测静默），近跳更小。
/// rdx/rbx 在 asm 内作临时寄存器——必须显式 clobber，否则编译器把局部值
/// 放进去会被踩坏（第一版未声明导致跳转后随机三重故障）。
/// 切 CR3（我们的 pml4：恒等 + LAPIC + HHDM + 内核高区）。切后恒等映射仍活，
/// OVMF 栈（ram 内）继续可用——start_aps 依赖这一点在切换后执行。
pub unsafe fn switch_cr3(pml4_phys: u64) {
    unsafe {
        core::arch::asm!(
            "mov cr3, rax",
            in("rax") pml4_phys,
            options(nomem, nostack, preserves_flags),
        )
    }
}

/// 已切 CR3 后调用；只负责最终 jmp（cli + cld + jmp）。
pub unsafe fn jump_kernel(entry_phys: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "cli",
            "cld",
            "jmp rax",
            in("rax") entry_phys,
            options(noreturn, nostack),
        )
    }
}
// ---------------------------------------------------------------- 最终交接

/// EBS 后串口直写（BootServices 不可用；serial 模块直接 port IO，无需参数）。
/// 这里的日志主要用于 QEMU 调试，验收断言不依赖（内核 banner 是终态锚）。
pub unsafe fn final_ebs_and_jump(
    bs: &efi::BootServices,
    hd: &mut boruix::Handover,
    pml4_phys: u64,
    entry: u64,
    rsdp_phys: u64,
) -> ! {
    unsafe {
    crate::serial::write(format_args!("[m4] final stage\n"));
    // 1) 最终内存映射（重试循环：size 出参不够时缓冲翻倍重取——规范 §7.2）。
    let mut buf = [0u8; efi::MEMMAP_BUF_SIZE];
    let mut key: usize = 0;
    let mut map_size: usize = 0;
    let mut dsize: usize = 0;
    let mut ok = false;
    for _ in 0..efi::EBS_MAX_RETRIES {
        let mut size: usize = buf.len();
        let mut dver: u32 = 0;
        let status = (bs.get_memory_map)(&mut size, buf.as_mut_ptr(), &mut key, &mut dsize, &mut dver);
        if status == efi::EFI_SUCCESS || !efi::is_error(status) {
            map_size = size;
            ok = true;
            break;
        }
    }
    if !ok {
        // EBS 前最后阶段失败：串口报错后停机。
        crate::serial::write(format_args!("[m4] final memmap failed\n"));
        loop { core::arch::asm!("hlt", options(nomem, nostack)); }
    }

    // 2) 转 Limine 语义 memmap（entries 数组放响应区后段）。
    let ndesc = map_size / dsize;
    let mut descs: [efi::MemoryDescriptor; 128] = core::array::from_fn(|_| efi::MemoryDescriptor {
        mem_type: 0, _pad: 0, physical_start: 0, virtual_start: 0, number_of_pages: 0, attribute: 0,
    });
    for i in 0..ndesc.min(128) {
        descs[i] = core::ptr::read(buf.as_ptr().add(i * dsize) as *const efi::MemoryDescriptor);
    }
    // entries 缓冲：单独 LoaderData 页。布局依据 brxlimine-rs MemmapResponse
    //   语义（lib.rs 626-627）：entries 是 entry_count 个 POINTERS 的数组
    //   （ArrayPtr<MemmapEntry> = NonNullPtr<NonNullPtr<T>>），内核逐槽解引用。
    //   M4 误填结构体连续数组 → 内核把 8B 槽当指针解引用出全垃圾
    //   （pmm max_phys 4GB / 0x0-0x0 段，boot 轮实锤）。新布局：
    //   页首 = 指针数组 [ptr; n]，页尾 = 结构体数组 [MemmapEntry; n]，
    //   指针 = 结构体槽位物理地址 + HHDM。8n + 24n ≤ 4096 → n ≤ 128。
    let mut entries_addr: u64 = 0;
    let st = (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, 1, &mut entries_addr);
    if efi::is_error(st) {
        loop { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
    core::slice::from_raw_parts_mut(entries_addr as *mut u8, 4096).fill(0);
    // 结构体数组放页尾（向下生长），容量 128。
    let struct_base = entries_addr + 4096 - 128 * 24;
    let entries = struct_base as *mut boruix::MemmapEntry;
    let kbase = hd.kaddr.physical_base;
    let klen = hd.file_struct.length;
    // 内核映像区间（物理）
    let kbase_al = kbase & !0x1F_FFFF;
    let kend = (kbase + klen + 0x1F_FFFF) & !0x1F_FFFF;
    // 帧缓冲物理基址（fb_struct.address 是 HHDM 虚地址——协议语义；剥离必须用
    // 物理：M5 曾误用虚地址当物理，产出 base=0xffff800080000000 的 Framebuffer
    // 条目，内核 pmm max_phys 飙到 17TB）。
    let fb_base = hd.fb_phys;
    let fb_size = if fb_base != 0 { 16 * 1024 * 1024 } else { 0 }; // 保守 16MiB 窗口
    let n = match convert_memmap(&descs[..ndesc.min(128)], entries, 128, kbase_al, kend, fb_base, fb_size) {
        Ok(n) => n,
        Err(_) => 0,
    };
    // 指针数组（页首）：第 i 项 = 结构体第 i 项的 HHDM 地址。
    let ptrs = entries_addr as *mut u64;
    for i in 0..n {
        let slot_phys = struct_base + (i as u64) * 24;
        core::ptr::write(ptrs.add(i), slot_phys + boruix::HHDM_OFFSET);
    }
    hd.memmap.entries = (entries_addr + boruix::HHDM_OFFSET) as *mut boruix::MemmapEntry;
    hd.memmap.entry_count = n as u64;
    crate::serial::write(format_args!("[m4] memmap converted n={}\n", n));

    // 3b) SMP: MADT walk + AP resource alloc + SmpInfo/Response fill (pre-EBS).
    let aps = crate::smp::prepare(bs, pml4_phys, rsdp_phys, hd);
    crate::serial::write(format_args!("[m4] smp prepared aps={}\n", aps));
    crate::serial::write(format_args!("[m4] smp page={:#x}\n", hd.smp_infos as u64));

    // 3c) 分配全部完成后**重取**最终内存映射（prepare 新分配的页必须
    //     以非 usable 类型进快照——否则内核 pmm 会覆盖 trampoline/栈
    //     /SmpInfo 页。第一次快照在分配前，必须作废重转。）
    {
        let mut size2: usize = buf.len();
        let mut key2: usize = 0;
        let mut dsz2: usize = 0;
        let mut dver2: u32 = 0;
        let st2 = (bs.get_memory_map)(&mut size2, buf.as_mut_ptr(), &mut key2, &mut dsz2, &mut dver2);
        if efi::is_error(st2) {
            crate::serial::write(format_args!("[m4] remap failed\n"));
            loop { core::arch::asm!("hlt", options(nomem, nostack)); }
        }
        let nd2 = size2 / dsz2;
        for i in 0..nd2.min(128) {
            descs[i] = core::ptr::read(buf.as_ptr().add(i * dsz2) as *const efi::MemoryDescriptor);
        }
        let n2 = match crate::handover::convert_memmap(&descs[..nd2.min(128)], entries, 128, kbase_al, kend, fb_base, fb_size) {
            Ok(n2) => n2,
            Err(_) => 0,
        };
        for i in 0..n2 {
            let slot_phys = struct_base + (i as u64) * 24;
            core::ptr::write(ptrs.add(i), slot_phys + boruix::HHDM_OFFSET);
        }
        hd.memmap.entries = (entries_addr + boruix::HHDM_OFFSET) as *mut boruix::MemmapEntry;
        hd.memmap.entry_count = n2 as u64;
        crate::serial::write(format_args!("[m4] memmap reconverted n={}\n", n2));
    }
    // 4) 内核映像里扫描请求标记（image_base 是物理；EBS 前恒等映射还活着——
    //    直接物理地址访问）。
    let filled = boruix::fill_requests(hd.kaddr.physical_base, hd.file_struct.length, hd);
    let base_ok = boruix::fill_base_revision(hd.kaddr.physical_base, hd.file_struct.length, 0); // rev 0 基线
    crate::serial::write(format_args!("[m4] requests filled={} baserev={}\n", filled, base_ok));

    // 5) EBS（map_key）→ 跳转。EBS 失败重试上限内重取（规范 §7.4 惯例）。
    crate::serial::write(format_args!("[m4] exiting boot services key={:#x}\n", key));
    for _ in 0..efi::EBS_MAX_RETRIES {
        let status = (bs.exit_boot_services)(crate::efi::image_handle_global(), key);
        if !efi::is_error(status) {
            crate::serial::write(format_args!("[m4] EBS ok, jumping entry={:#x}\n", entry));
            // 切到我们的页表（OVMF EBS 后栈在恒等映射内，本函数可继续）。
            switch_cr3(pml4_phys);
            // AP 启动必须在切页表后：trampoline 的 HHDM 轮询依赖我们的页表，
            // 且 OVMF EBS 流程会把已收编的 AP 重新挂起（QMP cpu_reset 实锤）。
            crate::smp::start_aps(hd);
            jump_kernel(entry);
        }
        // map 已变：重取
        let mut size: usize = buf.len();
        let mut dver: u32 = 0;
        let st2 = (bs.get_memory_map)(&mut size, buf.as_mut_ptr(), &mut key, &mut dsize, &mut dver);
        let _ = st2;
    }
    loop { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}