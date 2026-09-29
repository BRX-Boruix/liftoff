//! SMP（M6）：MADT 枚举 + AP trampoline + INIT-SIPI。
//!
//! 模拟 brxLimine 的 AP 启动协议：AP 经 INIT-SIPI 进入 trampoline，
//! 建长模式（BSP 同款 pml4：恒等 + HHDM + 内核高区），停在
//! `goto_address` 轮询；内核原子写非零后，AP 以 RDI=&SmpInfo、64KiB 栈
//! 跳转（brxlimine-rs lib.rs 537-543 语义）。

use crate::boruix;
use crate::efi;

/// LAPIC MMIO 基址（xAPIC；QEMU/OVMF 默认非 x2APIC）。
pub const LAPIC_BASE: u64 = 0xFEE0_0000;
const LAPIC_ICR_LOW: u64 = LAPIC_BASE + 0x300;
const LAPIC_ICR_HIGH: u64 = LAPIC_BASE + 0x310;
const LAPIC_ID_REG: u64 = LAPIC_BASE + 0x20;

/// MADT（signature "APIC"）表头（ACPI spec 5.2.11，SDT 通用头 36B）。
#[repr(C)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

const _: () = assert!(core::mem::size_of::<SdtHeader>() == 36);/// 从 RSDP 拷贝（物理地址）遍历 XSDT 找 MADT，返回 enabled LAPIC id 列表。
/// 全程物理地址直读（恒等映射期）。失败返回空表（SMP 退化单核）。
pub fn madt_lapic_ids(rsdp_phys: u64) -> [u32; MAX_CPUS] {
    let mut ids = [0u32; MAX_CPUS];
    let mut n = 0usize;
    if rsdp_phys == 0 {
        return ids;
    }
    unsafe {
        let rp = rsdp_phys as *const u8;
        let rev = core::ptr::read(rp.add(15));
        if rev < 2 {
            return ids; // XSDT 仅 ACPI 2.0+
        }
        let xsdt_phys = core::ptr::read_unaligned(rp.add(24) as *const u64);
        if xsdt_phys == 0 {
            return ids;
        }
        let h = &*(xsdt_phys as *const SdtHeader);
        let entries = ((h.length as usize) - 36) / 8;
        let base = xsdt_phys as usize + 36;
        for i in 0..entries {
            let t_phys = core::ptr::read_unaligned((base + i * 8) as *const u64);
            if t_phys == 0 {
                continue;
            }
            let th = &*(t_phys as *const SdtHeader);
            if th.signature != [0x41, 0x50, 0x49, 0x43] { // "APIC"
                continue;
            }
            // MADT entry 区（44 字节头之后）：type 0 = Processor Local APIC
            // entry：type u8 | len u8 | acpi_uid u8 | apic_id u8 | flags u32
            let mut off = 44usize;
            while off + 2 <= th.length as usize {
                let etype = core::ptr::read((t_phys as usize + off) as *const u8);
                let elen = core::ptr::read((t_phys as usize + off + 1) as *const u8);
                if elen < 2 {
                    break; // 规范不允许；防御死循环
                }
                if etype == 0 && elen >= 8 {
                    let flags = core::ptr::read_unaligned((t_phys as usize + off + 4) as *const u32);
                    if flags & 1 != 0 && n < MAX_CPUS { // bit0 processor enabled
                        let apic_id = core::ptr::read((t_phys as usize + off + 3) as *const u8);
                        ids[n] = apic_id as u32;
                        n += 1;
                    }
                }
                off += elen as usize;
            }
        }
    }
    ids
}

/// AP 上限（QEMU -smp 2..4 足够；超出截断）。
pub const MAX_CPUS: usize = 8;
/// 16-bit real mode entry bytes. abs32 slot relocated at build time.

// 三段 trampoline（对齐 brxLimine common/sys/smp_trampoline.asm_x86 的分段法）：
// 段1（16 位 @0）：clicld → lgdt[cs:gdtdesc]（16 位模式必须 disp16）→ PAE →
//   CR0=PE|ET → 远跳 0x08:段2。
// 段2（32 位 @0x40）：数据段 0x10 → EFER.LME → CR3 ← pml4 → CR0.PG → 远跳 0x18:段3。
// 段3（64 位 @0x80）：TRAMP64（标记 + goto 轮询 + 切栈/传参/跳转）。
// 补丁：段1 off32 @26；段2 栈槽 @0x50、pml4 @0x62、retf off32 @0x77。
const TRAMP1: [u8; 32] = [
    0xFA, 0xFC,                                        // cli; cld
    0x2E, 0x66, 0x0F, 0x01, 0x16, 0xA0, 0x01,          // lgdt [cs:0x1A0]（disp16 相对 CS）
    0x0F, 0x20, 0xE0, 0x83, 0xC8, 0x20, 0x0F, 0x22, 0xE0, // mov eax,cr4; or PAE; mov cr4,eax
    0xB8, 0x11, 0x00, 0x0F, 0x22, 0xC0,                // mov eax,0x11; mov cr0,eax  (PE|ET)
    0x66, 0xEA, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00          // ljmp 0x08:off32 (@26，跳到段2)
];

const TRAMP2: [u8; 60] = [
    0x66, 0xB8, 0x10, 0x00, 0x8E, 0xD8, 0x8E, 0xC0,    // mov ax,0x10; ds/es = 0x10
    0x8E, 0xE0, 0x8E, 0xE8, 0x8E, 0xD0,                // fs/gs/ss = 0x10
    0x8B, 0x25, 0x00, 0x00, 0x00, 0x00,                    // mov esp,[abs32] (@0x50：物理栈顶)
    0xB9, 0x80, 0x00, 0x00, 0xC0, 0x0F, 0x32,          // mov ecx,EFER; rdmsr
    0x0F, 0xBA, 0xE8, 0x08, 0x0F, 0x30,                // bts eax,8 (LME); wrmsr
    0xA1, 0x00, 0x00, 0x00, 0x00,                    // mov eax,[abs32] (@0x62：pml4 物理)
    0x0F, 0x22, 0xD8,                                  // mov cr3,eax
    0x0F, 0x20, 0xC0, 0x0D, 0x00, 0x00, 0x00, 0x80,    // mov eax,cr0; or PG
    0x0F, 0x22, 0xC0,                                  // mov cr0,eax
    0x6A, 0x18,                                        // push 0x18 (code64)
    0x68, 0x00, 0x00, 0x00, 0x00,                    // push off32 (@0x77 → 段3)
    0xCB                                               // retf：进 64 位段
];



// TRAMP64: 52 bytes at page+0x40.
//   [0..12)   mov qword [page+0x300], 0x5A5A1234  (存活标记，abs32 补丁于 [4..8))
//   [14..22)  movabs rax, &SmpInfo.goto_address  (补丁)
//   [22..25)  mov rax,[rax] / [25..28) test / [28..30) jz -18（回 movabs 指令起点）
//   [32..40)  movabs rsp,栈顶 (补丁) / [42..50) movabs rdi,&SmpInfo (补丁)
//   [50..52)  jmp rax
const TRAMP64: [u8; 52] = [
    0x48, 0xC7, 0x04, 0x25, 0x00, 0x00, 0x00, 0x00, 0x34, 0x12, 0x5A, 0x5A,
    0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0,
    0x48, 0x8B, 0x00,
    0x48, 0x85, 0xC0,
    0x74, 0xEE,   // jz -18 → 回到 movabs 指令起点（0x8C），不能落在立即数中间
    0x48, 0xBC, 0, 0, 0, 0, 0, 0, 0, 0,
    0x48, 0xBF, 0, 0, 0, 0, 0, 0, 0, 0,
    0xFF, 0xE0,
];


// goto_slot_hhdm at TRAMP64[2..10], stack at [12..20], smpinfo at [22..30].
// jz -9 at [17..19] loops back to the movabs (reload slot each poll).

/// AP param slots inside the trampoline page.
pub const TRAMP_PARAM_PML4: usize = 0x100;
/// 32 位段加载 ESP 用的**物理**栈顶槽（此时分页未开）。
pub const TRAMP_PARAM_STACK: usize = 0x108;
/// 页内 GDT（3 项）与 gdtdesc 偏移。
const GDT_OFF: usize = 0x180;
const GDTDESC_OFF: usize = 0x1A0;  // GDT 占 0x180..0x1A0，不得重叠
/// AP boot stack 64KiB (brxlimine-rs lib.rs 538).
pub const AP_STACK_PAGES: u64 = 16;
const INIT_DELAY_LOOPS: u32 = 200_000;
const SIPI_DELAY_LOOPS: u32 = 20_000;

/// ICR send via xAPIC MMIO: high half first, then low half.
unsafe fn icr_send(high: u32, low: u32) {
    unsafe {
        core::ptr::write_volatile(LAPIC_ICR_HIGH as *mut u32, high);
        core::ptr::write_volatile(LAPIC_ICR_LOW as *mut u32, low);
        let icr = LAPIC_ICR_LOW as *const u32;
        while core::ptr::read_volatile(icr) & (1 << 12) != 0 {}
    }
}

/// busy delay via port writes (no timer after EBS).
fn delay(loops: u32) {
    for _ in 0..loops {
        unsafe { core::arch::asm!("out 0x80, al", options(nomem, nostack, preserves_flags)); }
    }
}
pub unsafe fn prepare(bs: &efi::BootServices, pml4_phys: u64, rsdp_phys: u64, hd: &mut boruix::Handover) -> usize {
    unsafe {
    let ids = madt_lapic_ids(rsdp_phys);
    let bsp = (core::ptr::read_volatile(LAPIC_ID_REG as *const u32) >> 24) as u32;
    hd.smp.bsp_lapic_id = bsp;
    let bsp_info = &mut *hd.smp_infos;
    bsp_info.processor_id = bsp;
    bsp_info.lapic_id = bsp;
    bsp_info.reserved = 0;
    bsp_info.goto_address = 0;
    bsp_info.extra_argument = 0;
    *hd.smp_ptrs = (hd.smp_infos as u64 + boruix::HHDM_OFFSET) as *mut boruix::SmpInfo;
    let mut aps = 0usize;
    for i in 0..MAX_CPUS {
        let lid = ids[i];
        if lid == bsp { continue; }
        if lid == 0 && i > 0 { break; }
        if aps >= MAX_CPUS { break; }
        // 每 AP 一页：SIPI 向量 = 页号（必须 < 256 → 页 < 1MB），且必须避开
        // OVMF 的 AP 重定位缓冲（0x1F0000 曾被覆写，内存 dump 实证）。
        let mut tpage: u64 = 0x70000 + (aps as u64) * 0x1000;
        let st1 = (bs.allocate_pages)(efi::ALLOCATE_ADDRESS, efi::MEMORY_LOADER_DATA, 1, &mut tpage);
        if efi::is_error(st1) { break; }
        let mut spage: u64 = 0;
        let st2 = (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, AP_STACK_PAGES as usize, &mut spage);
        if efi::is_error(st2) { break; }
        let info = &mut *hd.smp_infos.add(1 + aps);
        info.processor_id = lid;
        info.lapic_id = lid;
        info.reserved = 0;
        info.goto_address = 0;
        info.extra_argument = 0;
        let page = tpage as usize;
        core::ptr::copy_nonoverlapping(TRAMP1.as_ptr(), page as *mut u8, TRAMP1.len());
        core::ptr::copy_nonoverlapping(TRAMP2.as_ptr(), (page + 0x40) as *mut u8, TRAMP2.len());
        core::ptr::copy_nonoverlapping(TRAMP64.as_ptr(), (page + 0x80) as *mut u8, TRAMP64.len());
        // GDT 四项：null / code32(0x08) / data(0x10) / code64(0x18)；gdtdesc 紧随。
        core::ptr::write_unaligned((page + GDT_OFF + 8) as *mut u64, 0x00CF_9A00_0000_FFFF);  // code32
        core::ptr::write_unaligned((page + GDT_OFF + 16) as *mut u64, 0x00CF_9200_0000_FFFF); // data
        core::ptr::write_unaligned((page + GDT_OFF + 24) as *mut u64, 0x00AF_9A00_0000_FFFF); // code64
        core::ptr::write_unaligned((page + GDTDESC_OFF) as *mut u16, 0x1F);                   // limit
        core::ptr::write_unaligned((page + GDTDESC_OFF + 2) as *mut u32, (page as u32) + (GDT_OFF as u32));
        // 段1 远跳 off32 @26；段2 栈槽 @0x50、pml4 @0x62、retf off32 @0x77。
        core::ptr::write_unaligned((page + 26) as *mut u32, (page as u32) + 0x40);
        core::ptr::write_unaligned((page + 0x50) as *mut u32, (page as u32) + (TRAMP_PARAM_STACK as u32));
        core::ptr::write_unaligned((page + 0x62) as *mut u32, (page as u32) + (TRAMP_PARAM_PML4 as u32));
        core::ptr::write_unaligned((page + 0x77) as *mut u32, (page as u32) + 0x80);
        // TRAMP64（@0x80）imm64 四处补丁：
        //   +4  marker abs32；+14 movabs rax,&SmpInfo.goto_address（字段 +16）；
        //   +32 movabs rsp,栈顶（HHDM 虚地址 64KiB）；+42 movabs rdi,&SmpInfo。
        let info_hhdm = hd.smp_infos as u64 + boruix::HHDM_OFFSET + ((1 + aps) as u64) * 32;
        core::ptr::write_unaligned((page + 0x80 + 4) as *mut u32, (page + 0x300) as u32);
        core::ptr::write_unaligned((page + 0x80 + 14) as *mut u64, info_hhdm + 16);
        core::ptr::write_unaligned((page + 0x80 + 32) as *mut u64, spage + AP_STACK_PAGES * 4096 + boruix::HHDM_OFFSET);
        core::ptr::write_unaligned((page + 0x80 + 42) as *mut u64, info_hhdm);
        // 参数槽（物理地址，分页开启前由段2读取）。
        core::ptr::write_unaligned((page + TRAMP_PARAM_PML4) as *mut u64, pml4_phys);
        core::ptr::write_unaligned((page + TRAMP_PARAM_STACK) as *mut u64, spage + AP_STACK_PAGES * 4096);
        // response pointer array entry
        *hd.smp_ptrs.add(1 + aps) = info_hhdm as *mut boruix::SmpInfo;
        TRAMP_PAGES[aps] = tpage;
        AP_LAPIC_IDS[aps] = lid;
        aps += 1;
    }
    hd.smp.cpu_count = (aps + 1) as u64;
    aps
    }
}

/// per-AP trampoline page phys / lapic id (filled by prepare).
pub static mut TRAMP_PAGES: [u64; MAX_CPUS] = [0; MAX_CPUS];
pub static mut AP_LAPIC_IDS: [u32; MAX_CPUS] = [0; MAX_CPUS];

/// EBS hou + CR3 switched: INIT-SIPI per AP (Intel SDM 8.4.4).
/// AP lands in trampoline, parks on goto_address polling (HHDM valid now).
pub unsafe fn start_aps(hd: &boruix::Handover) {
    unsafe {
        // SVR（0xF0）bit8 = APIC 软件使能：未使能时 ICR 写入被忽略（SDM 10.4.7），
        // 防御 OVMF 收尾留 PIC 模式；伪中断向量 0xFF，TPR（0x80）清 0 保证投递。
        let svr = core::ptr::read_volatile((LAPIC_BASE + 0xF0) as *const u32);
        core::ptr::write_volatile((LAPIC_BASE + 0xF0) as *mut u32, svr | 0x1FF);
        core::ptr::write_volatile((LAPIC_BASE + 0x80) as *mut u32, 0);
        let n = hd.smp.cpu_count as usize;
        let mut ap_index = 0usize;
        for i in 0..n {
            let info = &**hd.smp_ptrs.add(i);
            if (*info).lapic_id == hd.smp.bsp_lapic_id { continue; }
            // INIT IPI：delivery=5(INIT)、level assert、edge、目标 = 该 AP 的 LAPIC id
            let lid = AP_LAPIC_IDS[ap_index];
            icr_send(lid << 24, 0x00004500);
            delay(INIT_DELAY_LOOPS);
            let vec = (TRAMP_PAGES[ap_index] >> 12) as u32;
            icr_send(lid << 24, 0x00004600 | vec);
            delay(SIPI_DELAY_LOOPS);
            icr_send(lid << 24, 0x00004600 | vec);
            delay(SIPI_DELAY_LOOPS);
            // BSP 侧存活判定：AP 进长模式后第一件事就是写存活标记（页+0x300）。
            let marker_addr = (TRAMP_PAGES[ap_index] + 0x300) as *const u64;
            let mut alive = false;
            for _ in 0..200_000u32 {
                if core::ptr::read_volatile(marker_addr) == 0x5A5A1234 {
                    alive = true;
                    break;
                }
            }
            crate::serial::write(format_args!("[m6] ap{} alive={}\n", ap_index, alive));
            ap_index += 1;
        }
    }
}