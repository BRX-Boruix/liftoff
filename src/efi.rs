//! 手写最小 UEFI 绑定：M2a 触碰的表与协议。
//!
//! 布局以 UEFI Specification 2.10 为准，每个结构的大小用编译期断言钉死
//! （S04：平台差异不得依赖巧合）。字段顺序即内存偏移，禁止增删重排。
//!
//! 关于 dead_code 的 allow：协议表必须按规范**整段声明**——只声明用到的
//! 字段会使声明截断点之后的布局失真，后续成员无法继续追加。allow 只覆盖
//! 结构体字段（随里程碑逐步启用），不覆盖任何函数；每个里程碑结束时
//! 用到的字段必须真的被读到，否则属于死代码。
//! BootServices 只声明到 ExitBootServices（偏移 232..240）：其后的
//! InstallMultipleProtocolInterfaces / UninstallMultipleProtocolInterfaces
//! 是 C 变参函数，Rust 的 efiapi ABI 无法声明，触及对应能力时再以
//! 裸地址 + 手工调用约定处理。
#![allow(dead_code)]

use core::ffi::c_void;

// ---------------------------------------------------------------- 状态码

/// EFI_SUCCESS。
pub const EFI_SUCCESS: usize = 0;

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
}

const _: () = assert!(core::mem::size_of::<BootServices>() == 240);

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