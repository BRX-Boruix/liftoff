//! 手写最小 UEFI 绑定：M2a 触碰的表与协议。
//!
//! 布局以 UEFI Specification 2.10 为准，每个结构的大小用编译期断言钉死
//! （S04：平台差异不得依赖巧合）。字段顺序即内存偏移，禁止增删重排。
//!
//! 关于 dead_code 的 allow：协议表必须按规范**整段声明**——只声明用到的
//! 字段会使声明截断点之后的布局失真，后续成员无法继续追加。allow 只覆盖
//! 结构体字段（随里程碑逐步启用），不覆盖任何函数；每个里程碑结束时
//! 用到的字段必须真的被读到，否则属于死代码。
//! BootServices 声明到 LocateProtocol（偏移 320..328，size 328）：再往后的
//! InstallMultipleProtocolInterfaces / UninstallMultipleProtocolInterfaces
//! 是 C 变参函数，Rust 的 efiapi ABI 无法声明，触及对应能力时再以
//! 裸地址 + 手工调用约定处理。
#![allow(dead_code)]

use core::ffi::c_void;

// ---------------------------------------------------------------- 状态码

/// EFI_SUCCESS。
pub const EFI_SUCCESS: usize = 0;

/// 本映像的 UEFI 句柄（efi_main 入口单点写入，EBS 时经 image_handle_global 读）。
/// 引导期单核串行执行，无并发面。
static mut IMAGE_HANDLE: Handle = core::ptr::null_mut();
// 上面的 mutable static 读取在 2024 edition 需 unsafe 块——读口已包。

/// 单点登记（efi_main 首行调用）。
/// # Safety
/// 仅 efi_main 在其他任何 UEFI 调用前调用一次。
pub unsafe fn set_image_handle(h: Handle) {
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(IMAGE_HANDLE), h);
    }
}

/// EBS 需要的句柄读取口（handover 模块唯一消费者）。
pub fn image_handle_global() -> Handle {
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(IMAGE_HANDLE)) }
}

/// 错误位：EFI_STATUS 最高位。
pub const EFI_ERROR_BIT: usize = 0x8000_0000_0000_0000;

/// EFI_NOT_FOUND。
pub const EFI_NOT_FOUND: usize = EFI_ERROR_BIT | 14;

/// EFI_INVALID_PARAMETER。
pub const EFI_INVALID_PARAMETER: usize = EFI_ERROR_BIT | 2;

/// EFI_UNSUPPORTED。
pub const EFI_UNSUPPORTED: usize = EFI_ERROR_BIT | 3;

/// EFI_END_OF_FILE。Read 的正常终止状态：带错误位但不是失败
/// （§13.2，Read 在文件位置处于 EOF 时返回它）。
pub const EFI_END_OF_FILE: usize = EFI_ERROR_BIT | 18;

/// EFI_ALLOCATE_TYPE：AllocateAnyPages（§7.2，值 0——固件任选地址）。
pub const ALLOCATE_ANY_PAGES: u32 = 0;

/// EFI_ALLOCATE_TYPE：AllocateAddress（§7.2，值 2——钉死调用方给定地址）。
pub const ALLOCATE_ADDRESS: u32 = 2;

/// EFI_MEMORY_TYPE：EfiLoaderData（§7.2，值 2——引导加载器装载的映像数据，
/// ExitBootServices 后由内核继承，M4 交接语义）。
pub const MEMORY_LOADER_DATA: u32 = 2;

/// 判断状态码是否为错误。注意 END_OF_FILE 属于"错误位"但语义是正常终止。
pub fn is_error(status: usize) -> bool {
    status & EFI_ERROR_BIT != 0
}

// ---------------------------------------------------------------- 公共类型

/// EFI_HANDLE。
pub type Handle = *mut c_void;

/// CHAR16。
pub type Char16 = u16;

/// EFI_TABLE_HEADER：一切表与协议的公共头。
#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

const _: () = assert!(core::mem::size_of::<TableHeader>() == 24);

// ---------------------------------------------------------------- 引导服务

/// "BOOTSERV"，EFI_BOOT_SERVICES 的签名。
pub const BOOT_SERVICES_SIGNATURE: u64 = 0x5652_4553_544F_4F42;

/// EFI_BOOT_SERVICES 表，x64 布局，声明到 ExitBootServices（偏移 232..240）。
/// 成员顺序即规范 §4.4 定义顺序。
#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,

    // 任务优先级服务（§7.1）
    pub raise_tpl: unsafe extern "efiapi" fn(new_tpl: usize) -> usize,
    pub restore_tpl: unsafe extern "efiapi" fn(old_tpl: usize),

    // 内存服务（§7.2）
    pub allocate_pages:
        unsafe extern "efiapi" fn(kind: u32, memory_type: u32, pages: usize, memory: *mut u64) -> usize,
    pub free_pages: unsafe extern "efiapi" fn(memory: u64, pages: usize) -> usize,
    pub get_memory_map: unsafe extern "efiapi" fn(
        map_size: *mut usize,
        map: *mut u8, // M4 起 EFI_MEMORY_DESCRIPTOR
        map_key: *mut usize,
        desc_size: *mut usize,
        desc_version: *mut u32,
    ) -> usize,
    pub allocate_pool:
        unsafe extern "efiapi" fn(memory_type: u32, size: usize, buffer: *mut *mut c_void) -> usize,
    pub free_pool: unsafe extern "efiapi" fn(buffer: *mut c_void) -> usize,

    // 事件与定时器服务（§7.3-7.4）
    pub create_event: unsafe extern "efiapi" fn(
        kind: u32, notify_tpl: usize, notify_fn: *mut c_void, notify_ctx: *mut c_void, out: *mut *mut c_void,
    ) -> usize,
    pub set_timer: unsafe extern "efiapi" fn(event: *mut c_void, kind: u32, trigger_time: u64) -> usize,
    pub wait_for_event:
        unsafe extern "efiapi" fn(count: usize, events: *mut *mut c_void, out_index: *mut usize) -> usize,
    pub signal_event: unsafe extern "efiapi" fn(event: *mut c_void) -> usize,
    pub close_event: unsafe extern "efiapi" fn(event: *mut c_void) -> usize,
    pub check_event: unsafe extern "efiapi" fn(event: *mut c_void) -> usize,

    // 协议处理器（§7.5）
    pub install_protocol_interface:
        unsafe extern "efiapi" fn(handle: *mut Handle, protocol: *const c_void, kind: u32, iface: *mut c_void) -> usize,
    pub reinstall_protocol_interface:
        unsafe extern "efiapi" fn(handle: Handle, protocol: *const c_void, old: *mut c_void, new: *mut c_void) -> usize,
    pub uninstall_protocol_interface:
        unsafe extern "efiapi" fn(handle: Handle, protocol: *const c_void, iface: *mut c_void) -> usize,
    pub handle_protocol:
        unsafe extern "efiapi" fn(handle: Handle, protocol: *const c_void, out_iface: *mut *mut c_void) -> usize,
    /// 规范保留位（VOID *Reserved，位于 HandleProtocol 之后）。
    pub reserved: *mut c_void,
    pub register_protocol_notify:
        unsafe extern "efiapi" fn(protocol: *const c_void, event: *mut c_void, registration: *mut *mut c_void) -> usize,
    pub locate_handle:
        unsafe extern "efiapi" fn(search_type: u32, protocol: *const c_void, key: *mut c_void, size: *mut usize, buffer: *mut Handle) -> usize,
    pub locate_device_path:
        unsafe extern "efiapi" fn(protocol: *const c_void, device_path: *mut *mut c_void, out_device: *mut Handle) -> usize,
    pub install_configuration_table:
        unsafe extern "efiapi" fn(guid: *const c_void, table: *mut c_void) -> usize,

    // 镜像服务（§7.6）
    pub load_image: unsafe extern "efiapi" fn(
        boot_policy: bool, parent: Handle, path: *mut c_void, source: *mut c_void, source_size: usize, out_image: *mut Handle,
    ) -> usize,
    pub start_image:
        unsafe extern "efiapi" fn(image: Handle, exit_size: *mut usize, exit_data: *mut *mut Char16) -> usize,
    pub exit:
        unsafe extern "efiapi" fn(image: Handle, status: usize, exit_size: usize, exit_data: *mut Char16) -> usize,
    pub unload_image: unsafe extern "efiapi" fn(image: Handle) -> usize,
    pub exit_boot_services: unsafe extern "efiapi" fn(image: Handle, map_key: usize) -> usize,

    // 杂项服务（§7.8-7.9）
    pub get_next_monotonic_count: unsafe extern "efiapi" fn(count: *mut u64) -> usize,
    pub stall: unsafe extern "efiapi" fn(microseconds: usize),
    pub set_watchdog_timer: unsafe extern "efiapi" fn(
        watchdog_timeout: usize, watchdog_code: u64, data_size: usize, watchdog_data: *mut Char16,
    ) -> usize,

    // 控制器与协议打开（§7.10-7.11）
    pub connect_controller: unsafe extern "efiapi" fn(
        controller: Handle, driver_image: Handle, remaining_device_path: *mut c_void, recursive: bool,
    ) -> usize,
    pub disconnect_controller:
        unsafe extern "efiapi" fn(controller: Handle, driver_image: Handle, child: Handle) -> usize,
    pub open_protocol: unsafe extern "efiapi" fn(
        handle: Handle,
        protocol: *const c_void,
        out_interface: *mut *mut c_void,
        agent: Handle,
        controller: Handle,
        attributes: u32,
    ) -> usize,
    pub close_protocol:
        unsafe extern "efiapi" fn(handle: Handle, protocol: *const c_void, agent: Handle, controller: Handle) -> usize,
    pub open_protocol_information: unsafe extern "efiapi" fn(
        handle: Handle, protocol: *const c_void, entry_buffer: *mut *mut c_void, entry_count: *mut usize,
    ) -> usize,
    pub protocols_per_handle: unsafe extern "efiapi" fn(
        handle: Handle, protocol_buffer: *mut *mut *mut Guid, protocol_buffer_count: *mut usize,
    ) -> usize,
    pub locate_handle_buffer: unsafe extern "efiapi" fn(
        search_type: u32,
        protocol: *const c_void,
        search_key: *mut c_void,
        no_handles: *mut usize,
        buffer: *mut *mut Handle,
    ) -> usize,
    pub locate_protocol: unsafe extern "efiapi" fn(
        protocol: *const c_void, registration: *mut c_void, out_interface: *mut *mut c_void,
    ) -> usize,
}

const _: () = assert!(core::mem::size_of::<BootServices>() == 328);

// ---------------------------------------------------------------- 控制台输出

/// EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL（§12.3），10 成员 × 8 = 80 字节。
#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: unsafe extern "efiapi" fn(this: &SimpleTextOutput, extended: bool) -> usize,
    pub output_string:
        unsafe extern "efiapi" fn(this: &SimpleTextOutput, string: *const Char16) -> usize,
    pub test_string:
        unsafe extern "efiapi" fn(this: &SimpleTextOutput, string: *const Char16) -> usize,
    pub query_mode: unsafe extern "efiapi" fn(
        this: &SimpleTextOutput, mode_index: usize, columns: *mut usize, rows: *mut usize,
    ) -> usize,
    pub set_mode: unsafe extern "efiapi" fn(this: &SimpleTextOutput, mode_index: usize) -> usize,
    pub set_attribute: unsafe extern "efiapi" fn(this: &SimpleTextOutput, attr: usize) -> usize,
    pub clear_screen: unsafe extern "efiapi" fn(this: &SimpleTextOutput) -> usize,
    pub set_cursor_position: unsafe extern "efiapi" fn(
        this: &SimpleTextOutput, column: usize, row: usize,
    ) -> usize,
    pub enable_cursor: unsafe extern "efiapi" fn(this: &SimpleTextOutput, visible: bool) -> usize,
    pub mode: *mut c_void,
}

const _: () = assert!(core::mem::size_of::<SimpleTextOutput>() == 80);

// ---------------------------------------------------------------- 系统表

/// EFI_SYSTEM_TABLE（x64，120 字节）。
#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *mut Char16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut c_void,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut c_void, // M4 起具体化
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut c_void,
}

const _: () = assert!(core::mem::size_of::<SystemTable>() == 120);

impl SystemTable {
    /// 系统表签名，小端 "IBI SYST"。
    pub const SIGNATURE: u64 = 0x5453_5953_2049_4249;

    /// 校验签名并取表引用。不符返回 None，调用方只能走串口报告并停机。
    ///
    /// # Safety
    /// ptr 必须是固件传入的 EFI_SYSTEM_TABLE 指针。非空即按规范布局读取
    /// 头部签名；返回的引用生命周期覆盖引导服务阶段。
    pub unsafe fn from_ptr(ptr: *mut c_void) -> Option<&'static SystemTable> {
        if ptr.is_null() {
            return None;
        }
        let table = unsafe { &*(ptr as *const SystemTable) };
        if table.hdr.signature != Self::SIGNATURE {
            return None;
        }
        Some(table)
    }
}

// ---------------------------------------------------------------- 协议 GUID

/// GUID，16 字节，混合端序（data1/data2/data3 小端，data4 字节序）。
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<Guid>() == 16);

/// {5B1B31A1-9562-11D2-8E3F-00A0C969723B}，EFI_LOADED_IMAGE_PROTOCOL。
pub const LOADED_IMAGE_GUID: Guid = Guid {
    data1: 0x5B1B_31A1,
    data2: 0x9562,
    data3: 0x11D2,
    data4: [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

/// {964E5B22-6459-11D2-8E39-00A0C969723B}，EFI_SIMPLE_FILE_SYSTEM_PROTOCOL。
pub const SIMPLE_FILE_SYSTEM_GUID: Guid = Guid {
    data1: 0x964E_5B22,
    data2: 0x6459,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};
/// {964E5B21-6459-11D2-8E39-00A0C969723B}，EFI_BLOCK_IO_PROTOCOL。
pub const BLOCK_IO_GUID: Guid = Guid {
    data1: 0x964E_5B21,
    data2: 0x6459,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

// ---------------------------------------------------------------- 镜像与文件

/// EFI_LOADED_IMAGE_PROTOCOL（§8.2，96 字节）。
///
/// 首字段按规范是 Revision（UINT32），其前无 Type 字段；
/// EFI_TABLE_HEADER 只用于表级结构（SystemTable/BootServices/RuntimeServices），
/// 协议结构以 Revision 开头。
#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    pub file_path: *mut c_void,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: u32,
    pub image_data_type: u32,
    pub unload: unsafe extern "efiapi" fn(image: Handle) -> usize,
}

const _: () = assert!(core::mem::size_of::<LoadedImage>() == 96);

/// EFI_SIMPLE_FILE_SYSTEM_PROTOCOL（§13.1，16 字节）。
#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume:
        unsafe extern "efiapi" fn(this: &SimpleFileSystem, root: *mut *mut FileProtocol) -> usize,
}

const _: () = assert!(core::mem::size_of::<SimpleFileSystem>() == 16);

/// EFI_LOCATE_SEARCH_TYPE：ByProtocol（枚举值 2）。
pub const SEARCH_BY_PROTOCOL: u32 = 2;

/// Open 的打开模式：只读。
/// （完整集合：READ=1 | WRITE=2 | CREATE=0x8000_0000_0000_0000，§13.2）
pub const FILE_MODE_READ: u64 = 0x1;

/// EFI_FILE_PROTOCOL（§13.2，Revision + 10 成员 = 88 字节）。
/// Revision 不校验：Open/Read 语义自 0x0001_0000 起稳定。
#[repr(C)]
pub struct FileProtocol {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(
        this: &FileProtocol, new: *mut *mut FileProtocol, name: *const Char16, mode: u64, attrs: u64,
    ) -> usize,
    pub close: unsafe extern "efiapi" fn(this: &FileProtocol) -> usize,
    pub delete: unsafe extern "efiapi" fn(this: &FileProtocol) -> usize,
    pub read: unsafe extern "efiapi" fn(this: &FileProtocol, size: *mut usize, buffer: *mut u8) -> usize,
    pub write: unsafe extern "efiapi" fn(this: &FileProtocol, size: *mut usize, buffer: *const u8) -> usize,
    pub get_position: unsafe extern "efiapi" fn(this: &FileProtocol, position: *mut u64) -> usize,
    pub set_position: unsafe extern "efiapi" fn(this: &FileProtocol, position: u64) -> usize,
    pub get_info:
        unsafe extern "efiapi" fn(this: &FileProtocol, info: *const Guid, size: *mut usize, buffer: *mut u8) -> usize,
    pub set_info:
        unsafe extern "efiapi" fn(this: &FileProtocol, info: *const Guid, size: usize, buffer: *const u8) -> usize,
    pub flush: unsafe extern "efiapi" fn(this: &FileProtocol) -> usize,
}

const _: () = assert!(core::mem::size_of::<FileProtocol>() == 88);

/// 文件句柄守卫（S18）：Drop 即 Close，错误路径与正常路径共用一个释放点。
/// 关闭状态被忽略的前提是本工程只持有只读句柄；引入写句柄时必须重审。
pub struct FileGuard(pub *mut FileProtocol);

impl FileGuard {
    /// 收回裸指针（所有权转移给调用方，守卫不再负责关闭）。
    pub fn take(&mut self) -> *mut FileProtocol {
        let p = self.0;
        self.0 = core::ptr::null_mut();
        p
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ((*self.0).close)(&*self.0);
            }
        }
    }
}

// ---------------------------------------------------------------- 块设备

/// EFI_BLOCK_IO_MEDIA（§13.9，x64 布局）。
///
/// 字段顺序即规范定义：MediaId(u32) 后是 5 个 BOOLEAN(u8) 再 BlockSize，
/// 编译期断言钉死 LogicalPartition 的偏移——M2b 的光驱判定依赖 ReadOnly
/// 与 BlockSize==2048，M4 的 BootSource 依赖 LogicalPartition。
#[repr(C)]
pub struct BlockIoMedia {
    pub media_id: u32,
    pub removable_media: bool,
    pub media_present: bool,
    pub logical_partition: bool,
    pub read_only: bool,
    pub write_caching: bool,
    pub block_size: u32,
    pub io_align: u32,
    pub last_block: u64,
    // revision 2/3 扩展字段（LowestAlignedLba 等）本工程未触达，不声明；
    // 但 Media 结构由固件分配，读取止于 last_block 是安全的。
}

const _: () = assert!(core::mem::offset_of!(BlockIoMedia, logical_partition) == 6);
const _: () = assert!(core::mem::offset_of!(BlockIoMedia, read_only) == 7);
const _: () = assert!(core::mem::offset_of!(BlockIoMedia, block_size) == 12);
const _: () = assert!(core::mem::offset_of!(BlockIoMedia, last_block) == 24);

/// EFI_BLOCK_IO_PROTOCOL（§13.9，revision + media 指针 + 4 成员 = 48 字节）。
#[repr(C)]
pub struct BlockIo {
    pub revision: u64,
    pub media: *mut BlockIoMedia,
    pub reset: unsafe extern "efiapi" fn(this: &BlockIo, extended_verification: bool) -> usize,
    pub read_blocks: unsafe extern "efiapi" fn(
        this: &BlockIo, media_id: u32, lba: u64, buffer_size: usize, buffer: *mut u8,
    ) -> usize,
    pub write_blocks: unsafe extern "efiapi" fn(
        this: &BlockIo, media_id: u32, lba: u64, buffer_size: usize, buffer: *const u8,
    ) -> usize,
    pub flush_blocks: unsafe extern "efiapi" fn(this: &BlockIo) -> usize,
}

const _: () = assert!(core::mem::size_of::<BlockIo>() == 48);

/// 固件池句柄缓冲守卫（S18）：包住 LocateHandleBuffer 返回的缓冲。
/// Drop 即 FreePool；句柄数与缓冲同生共死，不暴露裸指针让调用方自理。
pub struct HandleBuffer<'a> {
    bs: &'a BootServices,
    handles: &'a mut [Handle],
    raw: *mut c_void,
}

impl<'a> HandleBuffer<'a> {
    /// 打包 LocateHandleBuffer 的两个出参。缓冲为空属固件异常，返回 None。
    /// # Safety
    /// buf 必须是本次 no_handles 对应的固件池缓冲，未做其他别名。
    pub unsafe fn wrap(
        bs: &'a BootServices,
        raw: *mut c_void,
        no_handles: usize,
    ) -> Option<HandleBuffer<'a>> {
        if raw.is_null() {
            return None;
        }
        let handles = unsafe { core::slice::from_raw_parts_mut(raw as *mut Handle, no_handles) };
        Some(HandleBuffer { bs, handles, raw })
    }

    pub fn handles(&self) -> &[Handle] {
        self.handles
    }
}

impl Drop for HandleBuffer<'_> {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // 释放失败无恢复路径：缓冲区只读，固件表若损坏，后续调用同样会失败。
            // （M4 引入内存映射后重审。）
            unsafe { (self.bs.free_pool)(self.raw) };
        }
    }
}

// ---------------------------------------------------------------- 辅助

/// 经 HandleProtocol 取协议接口。主流程只经此函数触达协议（S14）。
/// 返回 Err 的两种情形：固件报错；或 SUCCESS 但接口指针为空（规范不允许，
/// 属于防御分支，按 EFI_UNSUPPORTED 处置）。
pub fn protocol_of<T>(bs: &BootServices, handle: Handle, guid: &Guid) -> Result<*mut T, usize> {
    let mut iface: *mut c_void = core::ptr::null_mut();
    let status = unsafe {
        (bs.handle_protocol)(handle, guid as *const Guid as *const c_void, &mut iface)
    };
    if is_error(status) {
        return Err(status);
    }
    if iface.is_null() {
        return Err(EFI_UNSUPPORTED);
    }
    Ok(iface as *mut T)
}

/// 把字符串编码为 NUL 结尾的 UTF-16，写入调用方缓冲。
/// 缓冲不足返回 None（S09：宁可报错，不静默截断）。
pub fn wide_nul<'a>(s: &str, buf: &'a mut [Char16]) -> Option<&'a [Char16]> {
    let mut n = 0usize;
    for unit in s.encode_utf16() {
        if n >= buf.len() - 1 {
            return None;
        }
        buf[n] = unit;
        n += 1;
    }
    buf[n] = 0;
    Some(&buf[..n + 1])
}
// ---------------------------------------------------------------- GOP（M4）

/// EFI_GRAPHICS_OUTPUT_PROTOCOL（§11.9；3 槽 = 24 字节，Blt 槽不消费不声明
/// —— 不行，repr(C) 截断会错位：GOP 是 QueryMode/SetMode/Mode/Blt 四槽，
/// 全部声明才能保证 Mode 偏移 16 正确）。
#[repr(C)]
pub struct GraphicsOutput {
    pub query_mode: unsafe extern "efiapi" fn(
        this: &GraphicsOutput, mode_number: u32, size_of_info: *mut usize, info: *mut *mut GraphicsOutputModeInfo,
    ) -> usize,
    pub set_mode: unsafe extern "efiapi" fn(this: &GraphicsOutput, mode_number: u32) -> usize,
    pub blt: unsafe extern "efiapi" fn(
        this: &GraphicsOutput, buffer: *mut c_void, op: u32, sx: usize, sy: usize, dx: usize, dy: usize, w: usize, h: usize, delta: usize,
    ) -> usize,
    /// 规范槽位序：QueryMode/SetMode/Blt/Mode —— mode 在 24（首轮错排成 16，
    /// 断言拦不下（结构大小不变），运行期 info 指针读出垃圾才暴露）。
    pub mode: *mut GraphicsOutputMode,
}

const _: () = assert!(core::mem::size_of::<GraphicsOutput>() == 32);
const _: () = assert!(core::mem::offset_of!(GraphicsOutput, mode) == 24);

/// EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE（§11.9）。
#[repr(C)]
pub struct GraphicsOutputMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut GraphicsOutputModeInfo,
    pub size_of_info: usize,
    pub frame_buffer_base: u64,
    pub frame_buffer_size: u64,
}

const _: () = assert!(core::mem::offset_of!(GraphicsOutputMode, frame_buffer_base) == 24);
const _: () = assert!(core::mem::size_of::<GraphicsOutputMode>() == 40);

/// EFI_GRAPHICS_OUTPUT_MODE_INFORMATION（§11.9）。
#[repr(C)]
pub struct GraphicsOutputModeInfo {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    /// EFI_GRAPHICS_PIXEL_FORMAT：RGBX=0 BGRX=1 BitMask=2 BltOnly=3。
    pub pixel_format: u32,
    /// EFI_PIXEL_BITMASK。
    pub pixel_information: [u32; 4],
    pub pixels_per_scan_line: u32,
}

// version(4)+hres(4)+vres(4)+format(4)+bitmask(16)+scanline(4) = 36，对齐 4 无尾垫。
const _: () = assert!(core::mem::size_of::<GraphicsOutputModeInfo>() == 36);

/// pixel_format 常量（§11.9）。
pub const PIXEL_RGBX: u32 = 0;
pub const PIXEL_BGRX: u32 = 1;
pub const PIXEL_BITMASK: u32 = 2;
pub const PIXEL_BLT_ONLY: u32 = 3;

/// GOP GUID {9042A9DE-23DC-4A38-96FB-7ADED080516A}（§11.9）。
pub const GOP_GUID: Guid = Guid {
    data1: 0x9042A9DE,
    data2: 0x23DC,
    data3: 0x4A38,
    data4: [0x96, 0xFB, 0x7A, 0xDE, 0xD0, 0x80, 0x51, 0x6A],
};

// ---------------------------------------------------------------- 内存映射与 EBS（M4）

/// EBS 失败重试上限（§7.4：失败后 map 可能失效，须重取重试）。
pub const EBS_MAX_RETRIES: usize = 4;

/// 内存映射缓冲：OVMF 实测约 60..90 项 × 40B，64KiB 留 10 倍余量。
pub const MEMMAP_BUF_SIZE: usize = 64 * 1024;

/// EFI_MEMORY_DESCRIPTOR 消费切片（§7.2）＝ 40 字节。
#[repr(C)]
pub struct MemoryDescriptor {
    pub mem_type: u32,
    pub _pad: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

const _: () = assert!(core::mem::size_of::<MemoryDescriptor>() == 40);

/// EFI_MEMORY_TYPE（§7.2）。
pub const MEM_EFI_RESERVED: u32 = 0;
pub const MEM_EFI_LOADER_CODE: u32 = 1;
pub const MEM_EFI_LOADER_DATA: u32 = 2;
pub const MEM_EFI_BOOT_SERVICES_CODE: u32 = 3;
pub const MEM_EFI_BOOT_SERVICES_DATA: u32 = 4;
pub const MEM_EFI_RUNTIME_SERVICES_CODE: u32 = 5;
pub const MEM_EFI_RUNTIME_SERVICES_DATA: u32 = 6;
pub const MEM_EFI_CONVENTIONAL: u32 = 7;
pub const MEM_EFI_UNUSABLE: u32 = 8;
pub const MEM_EFI_ACPI_RECLAIM: u32 = 9;
pub const MEM_EFI_ACPI_NVS: u32 = 10;
pub const MEM_EFI_MEMORY_MAPPED_IO: u32 = 11;
pub const MEM_EFI_MEMORY_MAPPED_IO_PORT: u32 = 12;
pub const MEM_EFI_PAL_CODE: u32 = 13;
pub const MEM_EFI_PERSISTENT: u32 = 14;
/// LocateProtocol 单协议定位（§7.3；BootServices.locate_protocol 槽位）。
/// 返回协议实例指针（不存活校验——固件保证）。
pub fn locate_protocol<T>(bs: &BootServices, guid: &Guid) -> Result<*mut T, usize> {
    let mut out: *mut c_void = core::ptr::null_mut();
    let status = unsafe {
        (bs.locate_protocol)(guid as *const Guid as *const c_void, core::ptr::null_mut(), &mut out)
    };
    if is_error(status) {
        return Err(status);
    }
    Ok(out as *mut T)
}