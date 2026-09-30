//! `arch::paging::PageTable` 的 x86_64 实现：4 级页表 + 2 MiB 大页。
//!
//! 页表经直接映射（HHDM）访问：`DirectMap` 由调用方注入，因此宿主测试可以用内存缓冲
//! 构造"假 HHDM"，真实地走 PML4 -> PDPT -> PD 并断言页表项。

use arch::addr::{Alignment, PhysAddr, PhysFrame, VirtAddr};
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

    /// 取（必要时创建）下级页表帧。
    fn table_or_create(&mut self, frame: PhysFrame, index: u64) -> Result<PhysFrame, MapError> {
        let entry = self.read_entry(frame, index)?;
        if entry & PTE_PRESENT != 0 {
            return Ok(PhysFrame::containing(PhysAddr::new(entry & FRAME_ADDR_MASK)));
        }
        let fresh = self.allocator.allocate_zeroed().ok_or(MapError::OutOfMemory)?;
        let base = fresh.start_address().ok_or(MapError::Overflow)?.as_u64();
        self.write_entry(frame, index, base | PTE_PRESENT | PTE_WRITABLE)?;
        Ok(fresh)
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
            let pdpt = self.table_or_create(self.root, (v >> 39) & 0x1FF)?;
            let pd = self.table_or_create(pdpt, (v >> 30) & 0x1FF)?;
            self.write_entry(pd, (v >> 21) & 0x1FF, (p & LARGE_ADDR_MASK) | pte_flags | PTE_HUGE)?;
        }
        Ok(())
    }

    unsafe fn activate(&self) {
        let Some(phys) = self.root.start_address() else {
            return;
        };
        // SAFETY: 由调用方保证新页表仍映射当前正在执行的代码与栈（见 trait 的 SAFETY 契约）。
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) phys.as_u64(), options(nostack));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameAllocator, LARGE_PAGE_SIZE, X86PageTable};
    use arch::addr::{PhysAddr, PhysFrame, VirtAddr};
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
