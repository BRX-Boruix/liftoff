//! Lvl4 页表构建（M4）：Limine 语义的三区映射。
//!
//! 内核（mm::snapshot_kernel_root / paging::init / smp AP 路径）假设 CR3 已是
//! bootloader 建好的表，映射面：
//!   - 恒等低区：物理 RAM 全部（内核低区访问 / AP trampoline 语义）
//!   - HHDM：0xFFFF_8000_0000_0000 + phys 覆盖物理 RAM（mm::init 读 offset 后
//!     全部物理访问走这条）
//!   - 内核高区：0xFFFF_FFFF_8000_0000 起映射内核映像（exec）
//! 全部 2MB 大页 + PRESENT|WRITE（与 Limine 一致：无 NX 基线，内核页保护自管）。
//!
//! 页表页从 UEFI 固件分配（EfiLoaderData，EBS 后归属内核），分配数在结构上
//! 可预算：PDPT 每 512GB 一张 + PD 每 1GB 一张。

use crate::efi;

/// 页属性：PRESENT | WRITE（x86_64 PTE 位 0/1）。
pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITE: u64 = 1 << 1;
pub const PTE_HUGE: u64 = 1 << 7;

/// 2MB 大页粒度。
pub const HUGE_PAGE: u64 = 2 * 1024 * 1024;

/// 页表构建器：分配 PML4 并逐区映射。
pub struct PageTables {
    pub pml4_phys: u64,
}

/// 从 UEFI 分配一页（4KiB）清零页表页，返回物理地址。
fn alloc_table_page(bs: &efi::BootServices) -> Result<u64, usize> {
    let mut addr: u64 = 0;
    let status = unsafe {
        (bs.allocate_pages)(
            efi::ALLOCATE_ANY_PAGES,
            efi::MEMORY_LOADER_DATA,
            1,
            &mut addr,
        )
    };
    if efi::is_error(status) {
        return Err(0xA0); // A1 段：页表分配失败
    }
    unsafe {
        core::slice::from_raw_parts_mut(addr as *mut u8, 4096).fill(0);
    }
    Ok(addr)
}

/// 确保 PML4[P] 存在 PDPT、PDPT[i] 存在 PD（2MB 粒度），返回 PD 物理地址。
/// 中间页缺就分配（mmio.rs 同语义：主动分配并清零）。
unsafe fn ensure_pd(pml4: u64, pml4_idx: usize, pdpt_idx: usize, bs: &efi::BootServices) -> Result<u64, usize> {
    unsafe {
    let pml4_va = pml4 as *mut u64;
    let mut pdpt = pml4_va.add(pml4_idx).read_volatile() & 0x000F_FFFF_FFFF_F000;
    if pdpt == 0 {
        pdpt = alloc_table_page(bs)?;
        pml4_va.add(pml4_idx).write_volatile(pdpt | PTE_PRESENT | PTE_WRITE);
    }
    let pdpt_va = pdpt as *mut u64;
    let mut pd = pdpt_va.add(pdpt_idx).read_volatile() & 0x000F_FFFF_FFFF_F000;
    if pd == 0 {
        pd = alloc_table_page(bs)?;
        pdpt_va.add(pdpt_idx).write_volatile(pd | PTE_PRESENT | PTE_WRITE);
    }
    Ok(pd)
    }
}

/// 把 [phys_start, phys_end) 以 2MB 大页映射进指定 PML4 索引区，虚偏移 base_offset。
/// 区间必须 2MB 对齐（Limine 同约束；RAM 布局天然满足，映像区对齐 2MB）。
pub fn map_range(
    pml4_phys: u64,
    virt_offset: u64,
    phys_start: u64,
    phys_end: u64,
    bs: &efi::BootServices,
) -> Result<(), usize> {
    if phys_start % HUGE_PAGE != 0 || phys_end % HUGE_PAGE != 0 {
        return Err(0xA1); // 对齐违反
    }
    let mut cur = phys_start;
    while cur < phys_end {
        let virt = virt_offset + cur; // HHDM/恒等语义：virt = off + phys
        // 通用线性地址拆位（Lvl4）:
        let pml4_i = ((virt >> 39) & 0x1FF) as usize;
        let pdpt_i = ((virt >> 30) & 0x1FF) as usize;
        let pd_i = ((virt >> 21) & 0x1FF) as usize;
        let pd = unsafe { ensure_pd(pml4_phys, pml4_i, pdpt_i, bs)? };
        unsafe {
            (pd as *mut u64).add(pd_i).write_volatile(cur | PTE_PRESENT | PTE_WRITE | PTE_HUGE);
        }
        cur += HUGE_PAGE;
    }
    Ok(())
}

/// 内核高区映射（virt = 0xFFFFFFFF_80000000 + (phys - phys_base)，非线性）。
/// 逐 2MB 窗口映射：第 i 窗口 virt = kbase_virt + i*2MB，phys = kbase + i*2MB。
pub fn map_kernel_high(
    pml4_phys: u64,
    kbase_virt: u64,
    kbase_phys: u64,
    size: u64,
    bs: &efi::BootServices,
) -> Result<(), usize> {
    let pages = size.div_ceil(HUGE_PAGE);
    for i in 0..pages {
        let virt = kbase_virt + i * HUGE_PAGE;
        let phys = kbase_phys + i * HUGE_PAGE;
        let pml4_i = ((virt >> 39) & 0x1FF) as usize;
        let pdpt_i = ((virt >> 30) & 0x1FF) as usize;
        let pd_i = ((virt >> 21) & 0x1FF) as usize;
        let pd = unsafe { ensure_pd(pml4_phys, pml4_i, pdpt_i, bs)? };
        unsafe {
            (pd as *mut u64).add(pd_i).write_volatile(phys | PTE_PRESENT | PTE_WRITE | PTE_HUGE);
        }
    }
    Ok(())
}

/// 建整套页表。ram_top / kernel_base / kernel_size 都按 2MB 对齐（调用方保证）。
pub fn build(
    bs: &efi::BootServices,
    ram_top: u64,
    hhdm_top: u64,
    kernel_base: u64,
    kernel_size: u64,
    kernel_vbase: u64,
) -> Result<PageTables, usize> {
    let pml4 = alloc_table_page(bs)?;
    // 1) 恒等低区 [0, ram_top)。上界扩到 0xFF000000（覆盖 LAPIC 0xFEE00000）：
    //    EBS 回调期间固件 handler 仍写 LAPIC EOI（QMP 抓拍 CR2=0xFEE00020
    //    #PF 实锤），2MB 大页粒度下取整到 0xFF000000。
    const LAPIC_TOP: u64 = 0xFF00_0000;
    let low_top = ram_top.max(LAPIC_TOP);
    map_range(pml4, 0, 0, low_top, bs)?;
    // 2) HHDM [0xffff8000..., +ram_top)
    // HHDM 上界 = max(ram_top, hhdm_top)：帧缓冲 BAR 在 RAM 顶端之外，
    // 内核 [terminal] 直接写 HHDM+fb 会 #PF（QMP/内核异常报告实锤）。
    let h_top = ram_top.max(hhdm_top);
    map_range(pml4, crate::boruix::HHDM_OFFSET, 0, h_top, bs)?;
    // 3) 内核高区
    map_kernel_high(pml4, kernel_vbase, kernel_base, kernel_size, bs)?;
    Ok(PageTables { pml4_phys: pml4 })
}