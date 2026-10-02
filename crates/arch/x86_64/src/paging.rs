//! `arch::paging::PageTable` 的 x86_64 实现：4 级页表 + 2 MiB 大页 + 4 KiB 小页。
//!
//! 页表经直接映射（HHDM）访问：`DirectMap` 由调用方注入，因此宿主测试可以用内存缓冲
//! 构造"假 HHDM"，真实地走 PML4 -> PDPT -> PD 并断言页表项。

use arch::addr::{Alignment, PAGE_SIZE, PhysAddr, PhysFrame, VirtAddr};
use arch::hhdm::DirectMap;
use arch::paging::{FrameAllocator, MapError, PageFlags, PageTable, pages_for, validate_range};

/// 2 MiB 大页大小（本实现唯一的页粒度）。
pub const LARGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;

/// 2 MiB 对齐量。`LARGE_PAGE_SIZE` 是常量 2 的幂，构造必然成功。
const LARGE_ALIGN: Alignment = match Alignment::new_power_of_two(LARGE_PAGE_SIZE) {
    Some(align) => align,
    None => panic!("LARGE_PAGE_SIZE 必须是 2 的幂"),
};

const PTE_PRESENT: u64 = 1 << 0;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_HUGE: u64 = 1 << 7;
const PTE_NX: u64 = 1 << 63;
/// 4 KiB 帧基址掩码。
const FRAME_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// 2 MiB 大页物理基址掩码。
const LARGE_ADDR_MASK: u64 = 0x000F_FFFF_FFE0_0000;
/// 每级页表项数。
const ENTRIES_PER_TABLE: u64 = 512;
/// 1 GiB 大页大小（PDPT 项可表示的最大粒度）。
const HUGE_PAGE_SIZE: u64 = 1 << 30;

/// 某一级大页的物理基址掩码：掩掉页内偏移，再与可寻址位掩码相与。
///
/// 用算式而不是三个硬编码常量：`2 MiB` 与 `1 GiB` 的结果都能由它推出，
/// 少一处抄错的机会。
fn large_mask(page_size: u64) -> u64 {
    !(page_size - 1) & FRAME_ADDR_MASK
}

/// 本级项是否把 PS 位解释为「这是一个大页」；是则返回该大页大小。
///
/// **`ps_page_size` 为 `None` 时永远返回 `None`** —— 那一级没有 PS 位（PML4），
/// bit 7 是地址的一部分。抽成纯函数是为了能直接测这一档：真机才会踩到它，
/// 而宿主测试的帧地址恰好让 bit 7 恒为 0。
fn huge_at_level(ps_page_size: Option<u64>, entry: u64) -> Option<u64> {
    ps_page_size.filter(|_| entry & PTE_HUGE != 0)
}

/// 把沿途各级页表项折算成**语义权限**：取交集。
///
/// 只报叶项权限是常见错误 —— 会报出一个「可写」的地址，而实际写入被上级拒绝。
fn effective_flags(levels: &[u64]) -> PageFlags {
    let mut present = true;
    let mut writable = true;
    let mut executable = true;
    for entry in levels {
        present &= entry & PTE_PRESENT != 0;
        writable &= entry & PTE_WRITABLE != 0;
        executable &= entry & PTE_NX == 0;
    }
    let mut flags = PageFlags::none();
    if present {
        flags = flags.with(PageFlags::present());
    }
    if writable {
        flags = flags.with(PageFlags::writable());
    }
    if executable {
        flags = flags.with(PageFlags::executable());
    }
    flags
}


/// x86_64 页表（4 级 + 2 MiB 大页）。
pub struct X86PageTable<A> {
    root: PhysFrame,
    direct_map: DirectMap,
    allocator: A,
}

impl<A: FrameAllocator> X86PageTable<A> {
    /// 以根表帧、直接映射与帧来源构造。
    pub const fn new(root: PhysFrame, direct_map: DirectMap, allocator: A) -> Self {
        Self { root, direct_map, allocator }
    }

    /// 根表帧（`activate` 使用）。
    pub const fn root(&self) -> PhysFrame {
        self.root
    }

    fn entry_ptr(&self, frame: PhysFrame, index: u64) -> Result<*mut u64, MapError> {
        debug_assert!(index < ENTRIES_PER_TABLE, "页表索引越界");
        let phys = frame.start_address().ok_or(MapError::Overflow)?;
        let virt = self
            .direct_map
            .phys_to_virt(phys)
            .ok_or(MapError::TableNotAccessible)?;
        Ok(virt.as_u64() as *mut u64)
    }

    fn read_entry(&self, frame: PhysFrame, index: u64) -> Result<u64, MapError> {
        let ptr = self.entry_ptr(frame, index)?;
        // SAFETY: 指针由直接映射给出且覆盖该帧；index 已由 debug_assert 与调用方限制在 0..512。
        Ok(unsafe { core::ptr::read_volatile(ptr.add(index as usize)) })
    }

    fn write_entry(&self, frame: PhysFrame, index: u64, value: u64) -> Result<(), MapError> {
        let ptr = self.entry_ptr(frame, index)?;
        // SAFETY: 同 read_entry。
        unsafe { core::ptr::write_volatile(ptr.add(index as usize), value) };
        Ok(())
    }

    /// 取（必要时创建或**拆分**）下级页表帧。
    ///
    /// `ps_page_size` 是**本级项**用 PS 位表示的大页大小：
    /// * 检查 **PDPT** 项时是 `Some(1 GiB)`（PS=1 表示 1 GiB 页）
    /// * 检查 **PD** 项时是 `Some(2 MiB)`
    /// * 检查 **PML4** 项时是 `None` —— **PML4 项没有 PS 位**，bit 7 属于**地址域**。
    ///
    /// 若该位置已是**更大粒度**的映射（PS=1），这里把它**拆分**成下一级页表并如实
    /// 复制原有映射。**绝不能**把大页项里的地址当作页表帧地址使用 —— 那存的是**页帧
    /// 基址**，下钻等于往任意物理内存写 PTE（静默内存损坏）。
    ///
    /// **`None` 这一档是必需的，不是多余的分支**：在 PML4 项上检查 bit 7 会把地址里
    /// 恰好为 1 的那一位当成「这是大页」，于是对一张普通的页表帧做拆分，造出垃圾映射。
    /// 宿主测试用的帧地址全是 `0x10000000 + n*0x1000`（bit 7 恒为 0），**测不出这个错**；
    /// 真机的固件页地址可能带 bit 7 —— 这正是「宿主过、真机不过」的形态。
    fn table_or_create(
        &mut self,
        frame: PhysFrame,
        index: u64,
        ps_page_size: Option<u64>,
    ) -> Result<PhysFrame, MapError> {
        let entry = self.read_entry(frame, index)?;
        if entry & PTE_PRESENT != 0 {
            if let Some(page_size) = huge_at_level(ps_page_size, entry) {
                // 拆分后的子项大小 = 本级大页大小 / 每级项数。
                return self.split_large(frame, index, entry, page_size / ENTRIES_PER_TABLE);
            }
            return Ok(PhysFrame::containing(PhysAddr::new(entry & FRAME_ADDR_MASK)));
        }
        let fresh = self.allocator.allocate_zeroed().ok_or(MapError::OutOfMemory)?;
        let base = fresh.start_address().ok_or(MapError::Overflow)?.as_u64();
        self.write_entry(frame, index, base | PTE_PRESENT | PTE_WRITABLE)?;
        Ok(fresh)
    }

    /// 把已有的大页**拆分**成下一级页表，返回新表帧。
    ///
    /// **如实复制是核心契约**：每个子项映射 `base + slot * child_page_size`，并继承
    /// 父项的**可写**与 **NX** 位（present 由自己置）。少一项、或权限不对，原本可
    /// 访问的地址就会突然不可访问 —— 这类故障在真机上表现为随机复位，极难定位。
    fn split_large(
        &mut self,
        frame: PhysFrame,
        index: u64,
        entry: u64,
        child_page_size: u64,
    ) -> Result<PhysFrame, MapError> {
        let fresh = self.allocator.allocate_zeroed().ok_or(MapError::OutOfMemory)?;
        let base = entry & large_mask(child_page_size);
        let child_flags = PTE_PRESENT | (entry & (PTE_WRITABLE | PTE_NX));
        // 下级若仍是**大页**（拆分 1 GiB 得到 2 MiB 项），子项要带 PS 位。
        let child_huge = if child_page_size > PAGE_SIZE { PTE_HUGE } else { 0 };
        for slot in 0..ENTRIES_PER_TABLE {
            let addr = base + slot * child_page_size;
            self.write_entry(fresh, slot, addr | child_flags | child_huge)?;
        }
        // 父项改为指向新表：清 PS 位；**保留 NX**（非叶项的 NX 同样生效，丢掉就等于
        // 悄悄放开了执行权限）。
        let fresh_base = fresh.start_address().ok_or(MapError::Overflow)?.as_u64();
        self.write_entry(
            frame,
            index,
            fresh_base | PTE_PRESENT | PTE_WRITABLE | (entry & PTE_NX),
        )?;
        Ok(fresh)
    }

    /// 解除单个 4 KiB 页。
    ///
    /// **落在更大粒度映射内时报错**，不擅自拆分：拆分是「建立更细映射」的副作用，
    /// 让 `unmap` 顺手做它会把「解除」变成有分配行为的操作，失败语义立刻复杂化。
    /// 调用方若确实要解除大页区间，先建一条更细的映射（触发拆分）再调本方法。
    fn unmap_page(&mut self, v: u64) -> Result<(), MapError> {
        let pml4 = self.read_entry(self.root, (v >> 39) & 0x1FF)?;
        if pml4 & PTE_PRESENT == 0 {
            return Ok(());
        }
        let pdpt = self.read_entry(
            PhysFrame::containing(PhysAddr::new(pml4 & FRAME_ADDR_MASK)),
            (v >> 30) & 0x1FF,
        )?;
        if pdpt & PTE_PRESENT == 0 {
            return Ok(());
        }
        if pdpt & PTE_HUGE != 0 {
            return Err(MapError::UnsupportedGranularity);
        }
        let pd = self.read_entry(
            PhysFrame::containing(PhysAddr::new(pdpt & FRAME_ADDR_MASK)),
            (v >> 21) & 0x1FF,
        )?;
        if pd & PTE_PRESENT == 0 {
            return Ok(());
        }
        if pd & PTE_HUGE != 0 {
            return Err(MapError::UnsupportedGranularity);
        }
        let pt = PhysFrame::containing(PhysAddr::new(pd & FRAME_ADDR_MASK));
        // 写 0：present=0 即「不存在」。**页表帧本身不回收** —— 它仍被上级项引用，
        // 回收需要在每一级判断是否全空并改父项，属优化；当前无数据支撑其必要性（S32）。
        self.write_entry(pt, (v >> 12) & 0x1FF, 0)
    }
    /// 4 KiB 粒度映射。
    ///
    /// 与 `map_range`（2 MiB 大页）并列存在而不是替换它：大页是引导期的主要路径
    /// （表项少、TLB 压力小），4 KiB 用于**不满足大页对齐**的区间 —— 例如 Limine
    /// 语义下从 `0x1000` 起的低 4 GiB 恒等映射。此前缺这一能力，调用方被迫从
    /// `0` 起映射，把**页零**也映射了进去（与 Limine 的唯一已知偏差）。
    ///
    /// 页表逐 2 MiB 一张挂在 PD 上（PS=0），PTE 逐 4 KiB 填。权限位与 `map_range`
    /// 完全一致（present / writable / NX），不引入新的语义。
    pub fn map_range_pages(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        len: u64,
        flags: PageFlags,
    ) -> Result<(), MapError> {
        // 与 map_range 相同的参数校验，但按 4 KiB 对齐判定。
        validate_range(virt, phys, len, Alignment::PAGE)?;
        let mut pte_flags = PTE_PRESENT;
        if flags.is_writable() {
            pte_flags |= PTE_WRITABLE;
        }
        if !flags.is_executable() {
            pte_flags |= PTE_NX;
        }
        let mut remaining = len;
        let mut v = virt.as_u64();
        let mut p = phys.as_u64();
        while remaining > 0 {
            // 三级下钻：PML4 -> PDPT -> PD ->（本函数创建）PT。
            // 每张 4 KiB 页表覆盖 2 MiB，挂在 PD 上（PD 项 PS=0）。
            // PML4 项没有 PS 位 -> None；PDPT 项 PS 表示 1 GiB；PD 项 PS 表示 2 MiB。
            let pdpt = self.table_or_create(self.root, (v >> 39) & 0x1FF, None)?;
            let pd = self.table_or_create(pdpt, (v >> 30) & 0x1FF, Some(HUGE_PAGE_SIZE))?;
            let pt = self.table_or_create(pd, (v >> 21) & 0x1FF, Some(LARGE_PAGE_SIZE))?;
            let pt_index = (v >> 12) & 0x1FF;
            let pte = (p & FRAME_ADDR_MASK) | pte_flags;
            self.write_entry(pt, pt_index, pte)?;
            v += PAGE_SIZE;
            p += PAGE_SIZE;
            remaining -= PAGE_SIZE;
        }
        Ok(())
    }
}

impl<A: FrameAllocator> PageTable for X86PageTable<A> {
    fn map_range(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        len: u64,
        flags: PageFlags,
    ) -> Result<(), MapError> {
        // 两步校验：先用最小粒度（4 KiB）判“参数是否合法”，
        // 再判“本实现能不能做”（2 MiB），两者错误语义不同。
        validate_range(virt, phys, len, Alignment::PAGE)?;
        if !virt.is_aligned_to(LARGE_ALIGN) || !phys.is_aligned_to(LARGE_ALIGN) || len % LARGE_PAGE_SIZE != 0 {
            return Err(MapError::UnsupportedGranularity);
        }
        let pages = pages_for(len, LARGE_PAGE_SIZE).ok_or(MapError::Overflow)?;
        let mut pte_flags = PTE_PRESENT;
        if flags.is_writable() {
            pte_flags |= PTE_WRITABLE;
        }
        if !flags.is_executable() {
            pte_flags |= PTE_NX;
        }
        for page in 0..pages {
            // validate_range 已保证 virt + len 与 phys + len 不溢出，且 page < pages，故两处加法安全。
            let v = virt.as_u64() + page * LARGE_PAGE_SIZE;
            let p = phys.as_u64() + page * LARGE_PAGE_SIZE;
            let pdpt = self.table_or_create(self.root, (v >> 39) & 0x1FF, None)?;
            let pd = self.table_or_create(pdpt, (v >> 30) & 0x1FF, Some(HUGE_PAGE_SIZE))?;
            self.write_entry(pd, (v >> 21) & 0x1FF, (p & LARGE_ADDR_MASK) | pte_flags | PTE_HUGE)?;
        }
        Ok(())
    }

    fn unmap(&mut self, virt: VirtAddr, len: u64) -> Result<(), MapError> {
        if len == 0 {
            return Err(MapError::Empty);
        }
        let start = virt.as_u64();
        if start % PAGE_SIZE != 0 {
            return Err(MapError::MisalignedVirt);
        }
        if len % PAGE_SIZE != 0 {
            return Err(MapError::MisalignedLength);
        }
        let end = start.checked_add(len).ok_or(MapError::Overflow)?;
        let mut v = start;
        while v < end {
            self.unmap_page(v)?;
            v += PAGE_SIZE;
        }
        Ok(())
    }


    fn translate(&self, virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
        let v = virt.as_u64();
        // 逐级下钻；任一级不存在即「未映射」。`read_entry` 失败（帧不在直接映射内）
        // 也按未映射处理 —— 查不到就是查不到，不猜。
        let pml4 = self.read_entry(self.root, (v >> 39) & 0x1FF).ok()?;
        if pml4 & PTE_PRESENT == 0 {
            return None;
        }
        let pdpt = self
            .read_entry(PhysFrame::containing(PhysAddr::new(pml4 & FRAME_ADDR_MASK)), (v >> 30) & 0x1FF)
            .ok()?;
        if pdpt & PTE_PRESENT == 0 {
            return None;
        }
        if pdpt & PTE_HUGE != 0 {
            // 1 GiB 页：本实现不创建，但**如实处理**而不是当作未映射。
            let base = pdpt & 0x000F_FFFF_C000_0000;
            let phys = base | (v & 0x3FFF_FFFF);
            return Some((PhysAddr::new(phys), effective_flags(&[pml4, pdpt])));
        }
        let pd = self
            .read_entry(PhysFrame::containing(PhysAddr::new(pdpt & FRAME_ADDR_MASK)), (v >> 21) & 0x1FF)
            .ok()?;
        if pd & PTE_PRESENT == 0 {
            return None;
        }
        if pd & PTE_HUGE != 0 {
            // 2 MiB 大页：基址掩掉低 21 位，再或上页内偏移。
            let phys = (pd & LARGE_ADDR_MASK) | (v & (LARGE_PAGE_SIZE - 1));
            return Some((PhysAddr::new(phys), effective_flags(&[pml4, pdpt, pd])));
        }
        let pte = self
            .read_entry(PhysFrame::containing(PhysAddr::new(pd & FRAME_ADDR_MASK)), (v >> 12) & 0x1FF)
            .ok()?;
        if pte & PTE_PRESENT == 0 {
            return None;
        }
        let phys = (pte & FRAME_ADDR_MASK) | (v & (PAGE_SIZE - 1));
        Some((PhysAddr::new(phys), effective_flags(&[pml4, pdpt, pd, pte])))
    }

    unsafe fn activate(&self) {
        // 宿主测试目标上**不执行** `mov cr3`（特权指令，用户态直接
        // STATUS_PRIVILEGED_INSTRUCTION 崩溃）：宿主只验证映射构建逻辑，
        // 真正的激活在 UEFI 目标上发生 —— 这正是「宿主假固件 / 真机真固件」边界。
        #[cfg(target_os = "uefi")]
        {
            let Some(phys) = self.root.start_address() else {
                return;
            };
            // SAFETY: 由调用方保证新页表仍映射当前正在执行的代码与栈（见 trait 的 SAFETY 契约）。
            unsafe {
                core::arch::asm!("mov cr3, {}", in(reg) phys.as_u64(), options(nostack));
            }
        }
        #[cfg(not(target_os = "uefi"))]
        {
            let _ = &self.root;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::huge_at_level;

    #[test]
    fn the_ps_bit_is_only_honoured_at_levels_that_have_one() {
        // **这条测试守着一个只在真机才会踩到的错。** PML4 项**没有 PS 位** —— bit 7
        // 属于地址域，页表帧地址里它可能恰好为 1。若在 PML4 层检查它，就会把普通
        // 页表帧误判成 1 GiB 大页并拆分，造出垃圾映射。
        //
        // 宿主测试的帧地址全是 `0x10000000 + n*0x1000`（bit 7 恒为 0），所以**测不出**
        // 这个错；这里直接测判定函数本身。
        assert_eq!(huge_at_level(None, 0x80), None, "PML4 层不得解释 bit 7");
        assert_eq!(huge_at_level(None, 0x80 | 1), None, "present 也不能让它变成大页");
        assert_eq!(huge_at_level(Some(0x200000), 1), None, "没置 PS 就不是大页");
        assert_eq!(
            huge_at_level(Some(0x200000), 0x80 | 1),
            Some(0x200000),
            "PD 层置了 PS 才是 2 MiB 大页",
        );
        assert_eq!(huge_at_level(Some(0x4000_0000), 0x80 | 1), Some(0x4000_0000));
    }
    use super::{FrameAllocator, LARGE_PAGE_SIZE, X86PageTable};
    use arch::addr::{PAGE_SIZE, PhysAddr, PhysFrame, VirtAddr};
    use arch::hhdm::DirectMap;
    use arch::paging::{MapError, PageFlags, PageTable};
    use std::vec::Vec;

    const PHYS_BASE: u64 = 0x1000_0000;

    struct BufAlloc {
        words: Vec<u64>,
        next: u64,
        frames: u64,
    }

    impl BufAlloc {
        fn new(frames: u64) -> Self {
            Self { words: std::vec![0u64; (frames * 512) as usize], next: 0, frames }
        }

        fn allocated(&self) -> u64 {
            self.next
        }
    }

    impl FrameAllocator for BufAlloc {
        fn allocate_zeroed(&mut self) -> Option<PhysFrame> {
            if self.next >= self.frames {
                return None;
            }
            let frame = self.next;
            self.next += 1;
            Some(PhysFrame::containing(PhysAddr::new(PHYS_BASE + frame * 4096)))
        }
    }

    fn harness(frames: u64) -> (BufAlloc, DirectMap) {
        let alloc = BufAlloc::new(frames);
        let base = alloc.words.as_ptr() as u64;
        let offset = base.wrapping_sub(PHYS_BASE);
        let top = PHYS_BASE + frames * 4096;
        let dm = DirectMap::new(offset, top).expect("假 HHDM 区间有效");
        (alloc, dm)
    }

    fn read_entry(pt: &X86PageTable<BufAlloc>, frame: PhysFrame, index: u64) -> u64 {
        let phys = frame.start_address().expect("帧地址");
        let virt = pt.direct_map.phys_to_virt(phys).expect("帧在直接映射内");
        let ptr = virt.as_u64() as *const u64;
        // SAFETY: 指针落在测试缓冲内（假 HHDM 保证），index < 512。
        unsafe { core::ptr::read_volatile(ptr.add(index as usize)) }
    }

    fn pd_entry(pt: &X86PageTable<BufAlloc>, virt: VirtAddr) -> u64 {
        let v = virt.as_u64();
        let pml4 = read_entry(pt, pt.root, (v >> 39) & 0x1FF);
        assert_ne!(pml4 & 1, 0, "PML4 项应存在");
        let pdpt_frame = PhysFrame::containing(PhysAddr::new(pml4 & 0x000F_FFFF_FFFF_F000));
        let pdpt = read_entry(pt, pdpt_frame, (v >> 30) & 0x1FF);
        assert_ne!(pdpt & 1, 0, "PDPT 项应存在");
        let pd_frame = PhysFrame::containing(PhysAddr::new(pdpt & 0x000F_FFFF_FFFF_F000));
        read_entry(pt, pd_frame, (v >> 21) & 0x1FF)
    }

    #[test]
    fn maps_a_two_mib_page_with_the_huge_bit() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range(VirtAddr::new(0x1000_0000), PhysAddr::new(0x2000_0000), LARGE_PAGE_SIZE, PageFlags::present())
            .expect("映射成功");
        let pd = pd_entry(&pt, VirtAddr::new(0x1000_0000));
        assert_eq!(pd & (1 << 7), 1 << 7, "应使用 2 MiB 大页（PS 位）");
        assert_eq!(pd & 0x000F_FFFF_FFE0_0000, 0x2000_0000, "大页基址应为物理地址");
        assert_eq!(pd & 1, 1, "页应存在");
        assert_eq!(pd & 2, 0, "未请求可写");
        assert_eq!(pd >> 63, 1, "未请求可执行时应置 NX");
    }

    #[test]
    fn rejects_ranges_the_implementation_cannot_map() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        let a = LARGE_PAGE_SIZE;
        assert_eq!(pt.map_range(VirtAddr::new(0), PhysAddr::new(0), 0, PageFlags::present()), Err(MapError::Empty));
        assert_eq!(pt.map_range(VirtAddr::new(0x1001), PhysAddr::new(0), a, PageFlags::present()), Err(MapError::MisalignedVirt));
        assert_eq!(pt.map_range(VirtAddr::new(0), PhysAddr::new(0x1001), a, PageFlags::present()), Err(MapError::MisalignedPhys));
        assert_eq!(pt.map_range(VirtAddr::new(0), PhysAddr::new(0), a - 0x1000, PageFlags::present()), Err(MapError::UnsupportedGranularity));
        assert_eq!(pt.allocator.allocated(), 1, "参数非法时只应有根表帧");
    }


    #[test]
    fn reports_unsupported_granularity_separately_from_bad_arguments() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        // 4 KiB 对齐但不是 2 MiB 对齐：参数合法，实现不支持该粒度。
        assert_eq!(
            pt.map_range(VirtAddr::new(0x1000), PhysAddr::new(0x2000_0000), 0x1000, PageFlags::present()),
            Err(MapError::UnsupportedGranularity)
        );
        // 真正未对齐：仍应报参数错误。
        assert_eq!(
            pt.map_range(VirtAddr::new(0x1001), PhysAddr::new(0x2000_0000), 0x1000, PageFlags::present()),
            Err(MapError::MisalignedVirt)
        );
    }

    #[test]
    fn maps_four_kib_pages_with_pte_when_not_large_aligned() {
        // C1/DEBT-5：4 KiB 粒度。0x1000 起始（页零之后）正是 Limine 低 4 GiB
        // 映射的形态，也是当前实现做不到、只能从 0 起的缺口。
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range_pages(
            VirtAddr::new(0x1000),
            PhysAddr::new(0x1000),
            4 * LARGE_PAGE_SIZE,
            PageFlags::present(),
        )
        .expect("4 KiB 映射成功");
        // 从 0x1000 起映射 8 MiB，到 0x801000 止：跨入第 5 个 2 MiB 区间（v>>21 = 0..4），
        // 所以是根表 1 + PDPT 1 + PD 1 + **5 张 PT** = 8 帧。末页跨区间正是这个测试要抓的。
        assert_eq!(pt.allocator.allocated(), 8, "应分配 8 帧：根表 + PDPT + PD + 5 张 PT");
        // 第一张 4 KiB 页表应挂在 PD 的第 0 项，且**不**带 PS 位（PS=0 即 4 KiB 页）。
        let pd = pd_entry(&pt, VirtAddr::new(0x1000));
        assert_eq!(pd & 1, 1, "PD 项应存在（指向页表）");
        assert_eq!(pd & (1 << 7), 0, "PD 项不应带 PS 位（它指向 4 KiB 页表）");
        // PTE 应落在页表帧的第 1 项（virt 0x1000 -> PT index 1）。
        let pt_frame = PhysFrame::containing(PhysAddr::new(pd & 0x000F_FFFF_FFFF_F000));
        let pte = read_entry(&pt, pt_frame, 1);
        assert_eq!(pte & 1, 1, "PTE 应存在");
        assert_eq!(
            pte & 0x000F_FFFF_FFFF_F000,
            0x1000,
            "PTE 基址应为物理地址 0x1000",
        );
        assert_eq!(pte >> 63, 1, "未请求可执行时应置 NX");
    }

    #[test]
    fn four_kib_pages_cover_a_full_large_page_region() {
        // 跨 2 MiB 边界：4 KiB 页表必须逐 2 MiB 一张地建立。
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range_pages(
            VirtAddr::new(0x1000),
            PhysAddr::new(0x1000),
            4 * LARGE_PAGE_SIZE,
            PageFlags::present(),
        )
        .expect("跨边界 4 KiB 映射成功");
        for index in 0..4 {
            let v = VirtAddr::new(0x1000 + index * LARGE_PAGE_SIZE);
            let pd = pd_entry(&pt, v);
            assert_eq!(pd & 1, 1, "PD 项 {} 应存在", index);
            assert_eq!(pd & (1 << 7), 0, "PD 项 {} 不应带 PS 位", index);
        }
    }

    #[test]
    fn rejects_misaligned_four_kib_ranges_like_large_pages_do() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        assert_eq!(
            pt.map_range_pages(VirtAddr::new(0x1001), PhysAddr::new(0), 0x1000, PageFlags::present()),
            Err(MapError::MisalignedVirt),
        );
        assert_eq!(
            pt.map_range_pages(VirtAddr::new(0), PhysAddr::new(0x1001), 0x1000, PageFlags::present()),
            Err(MapError::MisalignedPhys),
        );
        assert_eq!(
            pt.map_range_pages(VirtAddr::new(0), PhysAddr::new(0), 0, PageFlags::present()),
            Err(MapError::Empty),
        );
    }

    #[test]
    fn the_full_low_4gib_sequence_matches_the_bootloader_plan() {
        // **按 `bring_up` 的真实顺序**组合：先 identity 大页（含低 2 MiB）-> 头部 4 KiB
        // （触发拆分）-> 主体大页 -> 显式解除页零。这条测试回答的是「映射逻辑本身
        // 对不对」，与真机环境（帧预算、固件行为）分开定位。
        let (mut alloc, dm) = harness(256);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        let rw = PageFlags::present().with(PageFlags::writable());
        // ① identity：低内存大页（`plan_identity` 的产物）。
        pt.map_range(VirtAddr::new(0), PhysAddr::new(0), LARGE_PAGE_SIZE, rw)
            .expect("identity 大页");
        // ② 头部 4 KiB：触发拆分。
        pt.map_range_pages(
            VirtAddr::new(0x1000),
            PhysAddr::new(0x1000),
            LARGE_PAGE_SIZE - 0x1000,
            rw,
        )
        .expect("头部 4 KiB");
        // ③ 主体大页。
        pt.map_range(
            VirtAddr::new(0x200000),
            PhysAddr::new(0x200000),
            LARGE_PAGE_SIZE,
            rw,
        )
        .expect("主体大页");
        // ④ 显式解除页零。
        pt.unmap(VirtAddr::new(0), PAGE_SIZE).expect("解除页零");

        assert!(pt.translate(VirtAddr::new(0)).is_none(), "页零必须已解除");
        assert!(pt.translate(VirtAddr::new(0x1000)).is_some(), "头部必须仍在");
        assert_eq!(
            pt.translate(VirtAddr::new(0x1000)).expect("头部可翻译").0.as_u64(),
            0x1000,
        );
        assert!(pt.translate(VirtAddr::new(0x200000)).is_some(), "主体必须仍在");
    }

    #[test]
    fn unmap_removes_a_four_kib_mapping() {
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range_pages(
            VirtAddr::new(0x1000),
            PhysAddr::new(0x5000),
            PAGE_SIZE,
            PageFlags::present(),
        )
        .expect("映射成功");
        assert!(pt.translate(VirtAddr::new(0x1000)).is_some(), "解除前应可翻译");
        pt.unmap(VirtAddr::new(0x1000), PAGE_SIZE).expect("解除成功");
        assert!(
            pt.translate(VirtAddr::new(0x1000)).is_none(),
            "解除后必须不可翻译 —— 这是 `unmap` 的全部意义",
        );
        // **幂等**：对已解除的地址再解除不算错误（目标状态就是「不存在」）。
        pt.unmap(VirtAddr::new(0x1000), PAGE_SIZE).expect("重复解除应成功");
    }

    #[test]
    fn unmap_rejects_ranges_inside_a_large_page_instead_of_splitting() {
        // 落在更大粒度映射内时报错，不擅自拆分 —— 那会把「解除」变成有分配行为的
        // 操作。调用方应先建更细的映射（触发拆分）再解除。
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range(
            VirtAddr::new(0x200000),
            PhysAddr::new(0x400000),
            LARGE_PAGE_SIZE,
            PageFlags::present(),
        )
        .expect("大页映射成功");
        assert_eq!(
            pt.unmap(VirtAddr::new(0x201000), PAGE_SIZE),
            Err(MapError::UnsupportedGranularity),
        );
        // 报错之后映射必须**原样还在** —— 拒绝不能有副作用。
        assert!(pt.translate(VirtAddr::new(0x201000)).is_some(), "拒绝不得破坏原映射");
    }

    #[test]
    fn unmap_rejects_bad_arguments() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        assert_eq!(pt.unmap(VirtAddr::new(0), 0), Err(MapError::Empty));
        assert_eq!(
            pt.unmap(VirtAddr::new(0x1001), PAGE_SIZE),
            Err(MapError::MisalignedVirt),
        );
        assert_eq!(
            pt.unmap(VirtAddr::new(0), PAGE_SIZE + 1),
            Err(MapError::MisalignedLength),
        );
    }

    #[test]
    fn translate_resolves_a_four_kib_mapping_with_its_flags() {
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range_pages(
            VirtAddr::new(0x1000),
            PhysAddr::new(0x5000),
            PAGE_SIZE,
            PageFlags::present().with(PageFlags::writable()),
        )
        .expect("映射成功");
        let (phys, flags) = pt.translate(VirtAddr::new(0x1000)).expect("应能翻译");
        assert_eq!(phys.as_u64(), 0x5000, "4 KiB 页的物理地址应逐位对上");
        assert!(flags.is_present(), "生效权限应含 present");
        assert!(flags.is_writable(), "生效权限应含 writable");
        assert!(!flags.is_executable(), "未请求可执行 -> NX 置位 -> 不可执行");
        // 未映射必须返回 None，**不能编一个地址出来**（S09）。
        assert!(pt.translate(VirtAddr::new(0x9000)).is_none(), "未映射应返回 None");
    }

    #[test]
    fn translate_resolves_a_two_mib_page_and_preserves_the_offset() {
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range(
            VirtAddr::new(0x200000),
            PhysAddr::new(0x400000),
            LARGE_PAGE_SIZE,
            PageFlags::present().with(PageFlags::executable()),
        )
        .expect("大页映射成功");
        // 大页内偏移必须保留：0x2A_BCDE 距 0x20_0000 为 0xA_BCDE。
        // 期望值写成**显式算术**而不是一个大字面量：`0x40_A_BCDE` 这种下划线位置会被
        // 解析成 `0x40ABCDE`（少一组 0），第一版就是这么写错、把正确实现判成失败的。
        let expected = 0x40_0000u64 + 0xA_BCDEu64;
        let (phys, flags) = pt.translate(VirtAddr::new(0x2A_BCDE)).expect("应能翻译");
        assert_eq!(phys.as_u64(), expected, "大页内偏移必须保留");
        assert!(flags.is_executable(), "请求了可执行 -> NX 未置位");
        assert!(!flags.is_writable(), "未请求可写");
    }

    #[test]
    fn splitting_preserves_page_zero_so_the_hybrid_needs_an_explicit_unmap() {
        // **这条测试记录一个推演结论，并用实现证实它。**
        //
        // 混合粒度（头部 0x1000..0x200000 用 4 KiB + 主体大页）看起来能消掉
        // 「多映射页零」的偏差。但拆分是**如实复制**原大页 —— 复制出的页表第 0 项
        // 仍然映射 `0..0x1000`，而头部映射只覆盖 `0x1000` 起，**不会碰第 0 项**。
        //
        // 所以：**混合粒度本身消不掉页零偏差，还需要显式的 unmap**（或等价的
        // 「清除区间」能力）。这正是 `unmap` 从「无消费者」变成「有消费者」的原因。
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        // 模拟 `plan_identity` 先为低内存建 2 MiB 大页。
        pt.map_range(
            VirtAddr::new(0),
            PhysAddr::new(0),
            LARGE_PAGE_SIZE,
            PageFlags::present().with(PageFlags::writable()),
        )
        .expect("大页映射成功");
        // 再建头部 4 KiB 映射（会触发拆分）。
        pt.map_range_pages(
            VirtAddr::new(0x1000),
            PhysAddr::new(0x1000),
            LARGE_PAGE_SIZE - 0x1000,
            PageFlags::present().with(PageFlags::writable()),
        )
        .expect("应拆分而不是报错");
        assert!(
            pt.translate(VirtAddr::new(0)).is_some(),
            "拆分如实复制，页零仍被映射 —— 所以混合粒度单独用不足以消掉页零偏差",
        );
    }

    #[test]
    fn splits_an_existing_large_page_and_preserves_the_mapping() {
        // 拆分：已有 2 MiB 大页时，要在其区间内建 4 KiB 页，必须把大页**拆成**
        // 一张页表，并**如实复制**原有映射 —— 少一项或权限不对，原本可访问的地址
        // 就会突然不可访问。
        let (mut alloc, dm) = harness(64);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range(
            VirtAddr::new(0x200000),
            PhysAddr::new(0x400000),
            LARGE_PAGE_SIZE,
            PageFlags::present().with(PageFlags::writable()),
        )
        .expect("大页映射成功");

        // 在大页区间内建一页 4 KiB —— 应当触发拆分。
        pt.map_range_pages(
            VirtAddr::new(0x201000),
            PhysAddr::new(0x9000),
            PAGE_SIZE,
            PageFlags::present(),
        )
        .expect("应拆分而不是报错");

        // ① 新页生效。
        let (phys, _) = pt.translate(VirtAddr::new(0x201000)).expect("新页可翻译");
        assert_eq!(phys.as_u64(), 0x9000, "新映射应覆盖拆分后的那一页");

        // ② **大页的其余部分必须原样保留** —— 这是拆分的核心契约。
        for offset in [0u64, 0x1000, 0x2000, 0x1FF000] {
            let v = 0x200000 + offset;
            if v == 0x201000 {
                continue;
            }
            let (phys, flags) = pt.translate(VirtAddr::new(v)).expect("拆分后仍应可翻译");
            assert_eq!(phys.as_u64(), 0x400000 + offset, "地址 {:#x} 的映射被拆分破坏了", v);
            assert!(flags.is_writable(), "权限必须在拆分中保留: {:#x}", v);
        }
    }


    #[test]
    fn reports_out_of_memory_when_the_frame_source_is_exhausted() {
        let (mut alloc, dm) = harness(1);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        assert_eq!(
            pt.map_range(VirtAddr::new(0), PhysAddr::new(0x2000_0000), LARGE_PAGE_SIZE, PageFlags::present()),
            Err(MapError::OutOfMemory)
        );
    }

    #[test]
    fn shares_intermediate_tables_across_pages() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        pt.map_range(VirtAddr::new(0), PhysAddr::new(0x2000_0000), 2 * LARGE_PAGE_SIZE, PageFlags::present())
            .expect("两页映射成功");
        assert_eq!(pt.allocator.allocated(), 3, "根表 + PDPT + PD，共 3 帧");
    }

    #[test]
    fn honours_writable_and_executable_flags() {
        let (mut alloc, dm) = harness(16);
        let root = alloc.allocate_zeroed().expect("根表帧");
        let mut pt = X86PageTable::new(root, dm, alloc);
        let flags = PageFlags::present().with(PageFlags::writable()).with(PageFlags::executable());
        pt.map_range(VirtAddr::new(0), PhysAddr::new(0x2000_0000), LARGE_PAGE_SIZE, flags).expect("映射成功");
        let pd = pd_entry(&pt, VirtAddr::new(0));
        assert_eq!(pd & 2, 2, "可写位应置位");
        assert_eq!(pd >> 63, 0, "可执行时应清 NX");
    }
}
