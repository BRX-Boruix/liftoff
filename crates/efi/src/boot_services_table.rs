//! `EFI_BOOT_SERVICES` 表前缀。
//!
//! 边界：只声明到 `ExitBootServices` 的前缀；**未使用的服务保持 `*mut c_void` 占位**，
//! 以保证后续字段偏移正确 —— 这里写错一个字段就是运行期跳到错误地址。
//! 偏移由宿主单测断言。

use crate::status::status_to_error;
use arch::addr::{PhysAddr, PhysFrame, PAGE_SIZE};
use arch::paging::FrameAllocator;
use crate::boot_services::GetMemoryMap;
use crate::types::{Handle, Status, TableHeader};
use core::ffi::c_void;

/// `ExitBootServices` 的签名。
pub type ExitBootServices = unsafe extern "efiapi" fn(image_handle: Handle, map_key: usize) -> Status;

/// `EFI_BOOT_SERVICES.Stall`：微秒级延时。
pub type Stall = unsafe extern "efiapi" fn(microseconds: usize) -> Status;

/// `AllocatePages` 的类型参数（UEFI 规范）：任意地址分配。
pub const ALLOCATE_ANY_PAGES: u32 = 0;
/// `AllocateMaxAddress`：`memory` 传入时是**允许的最高地址**，返回的页不高于它。
///
/// 需要它是因为**实模式只有 20 位寻址** ✗ —— AP 跳板必须落在 1 MiB 以下，
/// 否则 AP 醒来取不到第一条指令（表现为"发了 IPI 但 AP 不醒"）。
///
/// 【缺陷修正】这里曾经写成 **2** ✗。UEFI 规范里 `EFI_ALLOCATE_TYPE` 的枚举顺序是
/// `AllocateAnyPages`(0)、`AllocateMaxAddress`(**1**)、`AllocateAddress`(2) ✓ ——
/// 值 2 是 `AllocateAddress`，语义**正好相反**：它要求**正好分配在给定地址** ✗。
/// 于是"在 1 MiB 以下随便找一页"变成了"必须把 0xF_F000 这一页给我"：真机上 OVMF
/// 交不出那一页 → 分配失败 → `[liftoff] ap: 低页分配失败` → **一个 AP 都起不来** ✗。
///
/// **为什么原来的测试没抓到** ✗：它断言 `SEEN_TYPE == ALLOCATE_MAX_ADDRESS` ——
/// 把**实现**钉在常量上，却**没有把常量钉在规范上** ✓。两条都要有 ✓。
pub const ALLOCATE_MAX_ADDRESS: u32 = 1;
/// `AllocateAddress`：`memory` 是**要求的准确地址**（与上一个**不是**同一件事 ✗）。
/// 目前没有调用方，但显式列出枚举值，好让上面的常量有东西可对照 ✓。
pub const ALLOCATE_ADDRESS: u32 = 2;
/// 内存类型：引导器数据。
pub const EFI_LOADER_CODE: u32 = 1;
pub const EFI_LOADER_DATA: u32 = 2;

/// `AllocatePages` 的签名。
pub type AllocatePages =
    unsafe extern "efiapi" fn(allocate_type: u32, memory_type: u32, pages: usize, memory: *mut u64) -> Status;

/// `FreePages` 的签名。
pub type FreePages = unsafe extern "efiapi" fn(memory: u64, pages: usize) -> Status;

/// `HandleProtocol` 的签名。
pub type HandleProtocol = unsafe extern "efiapi" fn(
    handle: Handle,
    protocol: *const c_void,
    interface: *mut *mut c_void,
) -> Status;

/// `LocateHandle` 的签名。
pub type LocateHandle = unsafe extern "efiapi" fn(
    search_type: u32,
    protocol: *const c_void,
    search_key: *mut c_void,
    buffer_size: *mut usize,
    buffer: *mut Handle,
) -> Status;

/// `EFI_BOOT_SERVICES` 的前缀。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootServicesTable {
    /// 表头。
    pub hdr: TableHeader,
    /// 提升任务优先级（未使用）。
    pub raise_tpl: *mut c_void,
    /// 恢复任务优先级（未使用）。
    pub restore_tpl: *mut c_void,
    /// 分配页（未使用）。
    pub allocate_pages: AllocatePages,
    /// 释放页（未使用）。
    pub free_pages: FreePages,
    /// 取内存映射。
    pub get_memory_map: GetMemoryMap,
    /// 分配池内存（未使用）。
    pub allocate_pool: *mut c_void,
    /// 释放池内存（未使用）。
    pub free_pool: *mut c_void,
    /// 创建事件（未使用）。
    pub create_event: *mut c_void,
    /// 设置定时器（未使用）。
    pub set_timer: *mut c_void,
    /// 等待事件（未使用）。
    pub wait_for_event: *mut c_void,
    /// 触发事件（未使用）。
    pub signal_event: *mut c_void,
    /// 关闭事件（未使用）。
    pub close_event: *mut c_void,
    /// 检查事件（未使用）。
    pub check_event: *mut c_void,
    /// 安装协议接口（未使用）。
    pub install_protocol_interface: *mut c_void,
    /// 重装协议接口（未使用）。
    pub reinstall_protocol_interface: *mut c_void,
    /// 卸载协议接口（未使用）。
    pub uninstall_protocol_interface: *mut c_void,
    /// 查询句柄上的协议。
    pub handle_protocol: HandleProtocol,
    /// 保留字段。
    pub reserved: *mut c_void,
    /// 注册协议通知（未使用）。
    pub register_protocol_notify: *mut c_void,
    /// 按协议查找句柄。
    pub locate_handle: LocateHandle,
    /// 按设备路径查找（未使用）。
    pub locate_device_path: *mut c_void,
    /// 安装配置表（未使用）。
    pub install_configuration_table: *mut c_void,
    /// 装载映像（未使用）。
    pub load_image: *mut c_void,
    /// 启动映像（未使用）。
    pub start_image: *mut c_void,
    /// 退出当前映像（未使用）。
    pub exit: *mut c_void,
    /// 卸载映像（未使用）。
    pub unload_image: *mut c_void,
    /// 退出引导服务。
    pub exit_boot_services: ExitBootServices,
    /// 单调计数（未使用）。
    pub get_next_monotonic_count: *mut c_void,
    /// **微秒级延时**。
    ///
    /// 用于 IPI 之间**必须的等待**：INIT 之后要等约 **10 ms** 才发 SIPI ✗ ——
    /// 太早发 AP 会错过它（表现为"发了 IPI 但 AP 不醒"，串口上什么都没有 ✗）。
    ///
    /// 用固件的 `Stall` 而不是忙等自旋 ✓：延时**准确**，而忙等的时长取决于 CPU 速度 ✗。
    /// **可用性**：AP 启动发生在 `ExitBootServices` **之前**（Exit 在 `enter_kernel` 里 ✓），
    /// 所以引导服务仍然有效 ✓。
    pub stall: Stall,
}

#[cfg(test)]
mod tests {
    use super::BootServicesTable;
    use core::mem::offset_of;

    #[test]
    fn the_stall_offset_matches_the_spec() {
        // **断言完整值** ✓（本会话反复栽在"手算区间"上 ✗）。
        // 推导：hdr(24) + 之后每个指针 8 字节；`Stall` 紧跟 `GetNextMonotonicCount`。
        assert_eq!(offset_of!(BootServicesTable, exit_boot_services), 232);
        assert_eq!(offset_of!(BootServicesTable, get_next_monotonic_count), 240);
        assert_eq!(offset_of!(BootServicesTable, stall), 248);
    }

    #[test]
    fn memory_and_exit_offsets_match_the_spec() {
        assert_eq!(offset_of!(BootServicesTable, hdr), 0);
        assert_eq!(offset_of!(BootServicesTable, raise_tpl), 24);
        assert_eq!(offset_of!(BootServicesTable, allocate_pages), 40);
        assert_eq!(offset_of!(BootServicesTable, free_pages), 48);
        assert_eq!(offset_of!(BootServicesTable, get_memory_map), 56);
        assert_eq!(offset_of!(BootServicesTable, allocate_pool), 64);
        assert_eq!(offset_of!(BootServicesTable, free_pool), 72);
    }

    #[test]
    fn protocol_and_exit_offsets_match_the_spec() {
        assert_eq!(offset_of!(BootServicesTable, handle_protocol), 152);
        assert_eq!(offset_of!(BootServicesTable, locate_handle), 176);
        assert_eq!(offset_of!(BootServicesTable, exit_boot_services), 232);
    }
}

/// 基于固件 `AllocatePages` 的帧来源。
///
/// UEFI 的 `AllocatePages` **不保证**返回零化内存，而 `FrameAllocator::allocate_zeroed`
/// 的契约要求零化，故取回后自己清零 —— 页表帧若残留旧数据，会产生**伪映射**（比崩溃更难查）。
pub struct EfiFrameAllocator {
    allocate_pages: AllocatePages,
    /// **已成功交付的帧数** ✓ —— 用来回答"引导器运行期到底占了多少内存"（台账 §4.1）✓。
    ///
    /// **只数成功交付的** ✓：被拒的（状态错误、未对齐、越界）**不计** ✗ ——
    /// 否则"占了多少内存"会被虚报 ✗，而虚报的数字比没有数字更坏 ✓。
    allocated: usize,
}

impl EfiFrameAllocator {
    /// 以固件的 `AllocatePages` 指针构造。
    pub const fn new(allocate_pages: AllocatePages) -> Self {
        Self { allocate_pages, allocated: 0 }
    }

    /// 已成功交付的帧数 ✓。
    pub const fn allocated(&self) -> usize {
        self.allocated
    }
}

impl EfiFrameAllocator {
    /// 取一帧，**不高于** `max_address`（UEFI `AllocateMaxAddress` 语义）。
    ///
    /// 与 [`FrameAllocator::allocate_zeroed`] 同样自己清零（固件不保证零化 ✓），
    /// 并且**再验一次**固件交回的地址确实不高于上界 ✓ —— 固件理当遵守，但
    /// 越界的一页会让 AP 在实模式下取不到指令 ✗，宁可拒绝也不要一个"看起来成功"。
    ///
    /// `max_address` 是**允许的最高起始地址** ✓（调用方从 `AP_LOW_LIMIT - AP_PAGE_SIZE`
    /// 这类**推导**得出 ✓，不手写 ✗）。
    pub fn allocate_zeroed_below(&mut self, max_address: u64) -> Option<PhysFrame> {
        let mut address: u64 = max_address;
        // SAFETY: 由固件填写 `address`；其余参数按 UEFI 契约给出。
        let status = unsafe {
            (self.allocate_pages)(ALLOCATE_MAX_ADDRESS, EFI_LOADER_DATA, 1, &mut address)
        };
        if status_to_error(status).is_some() {
            return None;
        }
        if address % PAGE_SIZE != 0 {
            return None;
        }
        // 上界复核。**只比起始地址** ✓ —— 我第一版写成 `address + PAGE_SIZE > max + PAGE_SIZE`，
        // 而 `max = u64::MAX` 时右边**溢出成 `None`** ✗，`?` 于是把合法分配也拒了 ✗。
        // 这是本会话反复出现的"手算边界"同一个坑 ✓：能不加减就不加减 ✓。
        if address > max_address {
            return None;
        }
        // SAFETY: 固件刚分配的、页对齐的 4KiB 页，引导阶段该物理地址可直接访问。
        unsafe { core::ptr::write_bytes(address as *mut u8, 0, 4096) };
        // **只在这里计数** ✓ —— 走到这一行就是"真的交出去了" ✓。
        self.allocated += 1;
        Some(PhysFrame::containing(PhysAddr::new(address)))
    }
}

impl FrameAllocator for EfiFrameAllocator {
    fn allocate_zeroed(&mut self) -> Option<PhysFrame> {
        let mut address: u64 = 0;
        // SAFETY: 由固件填写 `address`；其余参数按 UEFI 契约给出。
        let status =
            unsafe { (self.allocate_pages)(ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, 1, &mut address) };
        if status_to_error(status).is_some() {
            return None;
        }
        // 固件必须交出**页对齐**的地址：若不对齐，`PhysFrame::containing` 会向下取整，
        // 我清的不是交出去的那一帧 —— 等于把未清零的内存当页表用（伪映射）。宁可拒绝。
        if address % PAGE_SIZE != 0 {
            return None;
        }
        // SAFETY: `address` 是固件刚分配的、页对齐的 4KiB 页，引导阶段该物理地址可直接访问。
        unsafe { core::ptr::write_bytes(address as *mut u8, 0, 4096) };
        // **只在这里计数** ✓ —— 走到这一行就是"真的交出去了" ✓。
        self.allocated += 1;
        Some(PhysFrame::containing(PhysAddr::new(address)))
    }
}

#[cfg(test)]
mod efi_frame_allocator_tests {
    use super::{
        ALLOCATE_ADDRESS, ALLOCATE_ANY_PAGES, ALLOCATE_MAX_ADDRESS, EFI_LOADER_CODE,
        EFI_LOADER_DATA, EfiFrameAllocator,
    };

    #[test]
    fn the_allocator_counts_only_the_frames_it_actually_handed_out() {
        // **只数成功交付的帧** ✓ —— 被拒的（越界/状态错误/未对齐）不得计入 ✗，
        // 否则"运行期占了多少内存"会被虚报 ✗（台账 §4.1 要的就是这个数）。
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        MISALIGN.store(0, Ordering::SeqCst);
        let mut allocator = EfiFrameAllocator::new(fake_alloc);
        assert_eq!(allocator.allocated(), 0, "一开始一个都没交出去");
        let _ = allocator.allocate_zeroed_below(u64::MAX).expect("上界足够大");
        assert_eq!(allocator.allocated(), 1, "成功一帧就记一帧");
        // 上界远小于夹具返回的宿主地址 → 必被拒 ✓ → **不得计入** ✓。
        let _ = allocator.allocate_zeroed_below(0x10);
        assert_eq!(allocator.allocated(), 1, "被拒的分配不得计入");
    }
    use arch::paging::FrameAllocator;
    use crate::types::{Status, SUCCESS, DEVICE_ERROR};
    use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// 假固件看到的**上界入参**（`AllocateMaxAddress` 语义：入参即上界）。
    static SEEN_MAX: AtomicU64 = AtomicU64::new(0);

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static SEEN_TYPE: AtomicUsize = AtomicUsize::new(0);
    static SEEN_MEMTYPE: AtomicUsize = AtomicUsize::new(0);
    static SEEN_PAGES: AtomicUsize = AtomicUsize::new(0);
    static FAIL: AtomicUsize = AtomicUsize::new(0);
    /// 让假固件故意交出不对齐地址（0 = 正常，非 0 = 加偏移）。
    static MISALIGN: AtomicUsize = AtomicUsize::new(0);

    /// 假 AllocatePages：把一帧真实宿主内存交出去，并记录收到的参数。
    /// SAFETY: 调用方按 UEFI 契约传入有效指针；本实现写入调用方给的 `memory`。
    unsafe extern "efiapi" fn fake_alloc(
        allocate_type: u32,
        memory_type: u32,
        pages: usize,
        memory: *mut u64,
    ) -> Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        SEEN_TYPE.store(allocate_type as usize, Ordering::SeqCst);
        // SAFETY: 入参指针按 UEFI 契约有效。
        SEEN_MAX.store(unsafe { *memory }, Ordering::SeqCst);
        SEEN_MEMTYPE.store(memory_type as usize, Ordering::SeqCst);
        SEEN_PAGES.store(pages, Ordering::SeqCst);
        if FAIL.load(Ordering::SeqCst) != 0 {
            return DEVICE_ERROR;
        }
        // 交出宿主上一块**页对齐**、内容非零的缓冲，用来验证「必须清零」。
        #[repr(align(4096))]
        struct Aligned([u8; 8192]);
        static mut POOL: Aligned = Aligned([0xA5; 8192]);
        // SAFETY: 只取地址，不解引用；夹具在测试期间存活。
        let base = unsafe { core::ptr::addr_of_mut!(POOL.0) as *mut u8 as u64 };
        assert_eq!(base % 4096, 0, "测试夹具必须页对齐");
        unsafe { *memory = base + MISALIGN.load(Ordering::SeqCst) as u64 };
        SUCCESS
    }

    #[test]
    fn the_allocate_type_and_memory_type_values_match_the_uefi_spec() {
        // 【缺陷】`ALLOCATE_MAX_ADDRESS` 曾经是 2 ✗ —— 规范里它是 **1**，2 是 `AllocateAddress`。
        // 这一个数字让"在 1 MiB 以下找一页"变成了"必须给我 0xF_F000 这一页" ✗，
        // 真机上表现为 `[liftoff] ap: 低页分配失败`、**一个 AP 都起不来** ✗。
        // 所以这里钉的是**规范值**，与上面那条"实现是否用了这个常量"是**两件事** ✓。
        assert_eq!(ALLOCATE_ANY_PAGES, 0);
        assert_eq!(ALLOCATE_MAX_ADDRESS, 1, "AllocateMaxAddress 是 1，不是 2");
        assert_eq!(ALLOCATE_ADDRESS, 2, "2 是 AllocateAddress：要求准确地址");
        // 内存类型同理（`EFI_MEMORY_TYPE` 的枚举顺序）。
        assert_eq!(EFI_LOADER_CODE, 1);
        assert_eq!(EFI_LOADER_DATA, 2);
    }

    #[test]
    fn a_bounded_allocation_asks_with_the_max_address_type_and_passes_the_bound_through() {
        // **断言完整值，不手算区间** ✓（本会话反复栽在"手算边界"上 ✗）。
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        MISALIGN.store(0, Ordering::SeqCst);
        let mut allocator = EfiFrameAllocator::new(fake_alloc);
        // 上界取 `u64::MAX`：只为验证**参数确实被透传**，与具体数值无关。
        let frame = allocator.allocate_zeroed_below(u64::MAX).expect("上界足够大");
        assert_eq!(SEEN_TYPE.load(Ordering::SeqCst), ALLOCATE_MAX_ADDRESS as usize);
        assert_eq!(SEEN_MAX.load(Ordering::SeqCst), u64::MAX, "上界必须原样透传给固件");
        assert_eq!(SEEN_PAGES.load(Ordering::SeqCst), 1, "一帧 = 一个 4KiB 页");
        assert_eq!(SEEN_MEMTYPE.load(Ordering::SeqCst), EFI_LOADER_DATA as usize);
        // 与无上界那条同样必须**已清零** ✓。
        let base = frame.start_address().expect("帧地址可算").as_u64();
        // SAFETY: 固件刚交出的页，测试期间可读。
        let byte = unsafe { core::ptr::read_volatile(base as *const u8) };
        assert_eq!(byte, 0, "交付的帧必须已清零");
    }

    #[test]
    fn a_frame_the_firmware_returns_above_the_bound_is_rejected() {
        // **这是本方法的全部意义**：越界的一页会让 AP 在实模式下取不到指令 ✗。
        // 夹具交回的是宿主上一块**高地址**缓冲 ✓ —— 用一个很小的上界就能逼出拒绝 ✓。
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        MISALIGN.store(0, Ordering::SeqCst);
        let mut allocator = EfiFrameAllocator::new(fake_alloc);
        // 0xFF000 = 1 MiB - 4 KiB：跳板页的**推导**上界（整页都要 < 1 MiB ✓）。
        assert!(
            allocator.allocate_zeroed_below(0x000F_F000).is_none(),
            "固件交出越界地址时必须拒绝，而不是当作成功"
        );
    }

    #[test]
    fn a_frame_comes_back_zeroed_and_the_call_uses_the_spec_arguments() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        let mut allocator = EfiFrameAllocator::new(fake_alloc);
        let frame = allocator.allocate_zeroed().expect("应取到一帧");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(SEEN_TYPE.load(Ordering::SeqCst), ALLOCATE_ANY_PAGES as usize);
        assert_eq!(SEEN_MEMTYPE.load(Ordering::SeqCst), EFI_LOADER_DATA as usize);
        assert_eq!(SEEN_PAGES.load(Ordering::SeqCst), 1, "一帧 = 一个 4KiB 页");
        // 交付的帧必须是**已清零**的：AllocatePages 本身不保证零化。
        let base = frame.start_address().expect("帧地址可算").as_u64();
        let bytes = unsafe { core::slice::from_raw_parts(base as *const u8, 4096) };
        assert!(bytes.iter().all(|b| *b == 0), "取回的帧必须被清零");
    }

    #[test]
    fn a_misaligned_address_is_rejected() {
        FAIL.store(0, Ordering::SeqCst);
        MISALIGN.store(8, Ordering::SeqCst);
        let mut allocator = EfiFrameAllocator::new(fake_alloc);
        assert!(
            allocator.allocate_zeroed().is_none(),
            "固件交出不对齐地址时必须拒绝：否则清的不是交出去的那一帧"
        );
        MISALIGN.store(0, Ordering::SeqCst);
    }

    #[test]
    fn a_failing_allocate_pages_yields_none() {
        FAIL.store(1, Ordering::SeqCst);
        let mut allocator = EfiFrameAllocator::new(fake_alloc);
        assert!(allocator.allocate_zeroed().is_none(), "固件拒绝时应返回 None");
    }
}