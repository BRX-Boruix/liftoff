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

/// `AllocatePages` 的类型参数（UEFI 规范）：任意地址分配。
pub const ALLOCATE_ANY_PAGES: u32 = 0;
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
}

#[cfg(test)]
mod tests {
    use super::BootServicesTable;
    use core::mem::offset_of;

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
}

impl EfiFrameAllocator {
    /// 以固件的 `AllocatePages` 指针构造。
    pub const fn new(allocate_pages: AllocatePages) -> Self {
        Self { allocate_pages }
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
        Some(PhysFrame::containing(PhysAddr::new(address)))
    }
}

#[cfg(test)]
mod efi_frame_allocator_tests {
    use super::{ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, EfiFrameAllocator};
    use arch::paging::FrameAllocator;
    use crate::types::{Status, SUCCESS, DEVICE_ERROR};
    use core::sync::atomic::{AtomicUsize, Ordering};

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