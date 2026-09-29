//! 最小 UEFI 绑定：只声明 Liftoff 实际触碰的表与协议。
//!
//! 字段布局以 UEFI 规范为准（含保留字段），结构体大小必须与规范一致。

use core::ffi::c_void;

/// UEFI 状态码：成功。
pub const EFI_SUCCESS: usize = 0;

/// UEFI 状态码：函数执行中发生错误。
pub const EFI_ERROR: usize = 1 << 63;

/// 状态码是否为错误（最高位为 1）。
pub fn is_error(status: usize) -> bool {
    status & EFI_ERROR != 0
}

/// GUID，16 字节，按小端序逐字节比较。
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    /// EFI_SIMPLE_FILE_SYSTEM_PROTOCOL。
    pub const SIMPLE_FILE_SYSTEM: Guid = Guid {
        data1: 0x0964e5b22,
        data2: 0x6459,
        data3: 0x11d2,
        data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
    };

    /// EFI_LOADED_IMAGE_PROTOCOL。
    pub const LOADED_IMAGE: Guid = Guid {
        data1: 0x5b1b31a1,
        data2: 0x9562,
        data3: 0x11d2,
        data4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
    };

    /// EFI_FILE_INFO。
    pub const FILE_INFO: Guid = Guid {
        data1: 0x09576e92,
        data2: 0x6d3f,
        data3: 0x11d2,
        data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
    };
}

/// CHAR16。
pub type Char16 = u16;

/// EFI_TABLE_HEADER：所有表与协议的公共头。
#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub _reserved: u32,
}

/// EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL。
#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: unsafe extern "efiapi" fn(this: &SimpleTextOutput, extended: bool) -> usize,
    pub output_string:
        unsafe extern "efiapi" fn(this: &SimpleTextOutput, string: *const Char16) -> usize,
    pub test_string:
        unsafe extern "efiapi" fn(this: &SimpleTextOutput, string: *const Char16) -> usize,
    pub query_mode: unsafe extern "efiapi" fn(
        this: &SimpleTextOutput,
        mode_index: usize,
        columns: *mut usize,
        rows: *mut usize,
    ) -> usize,
    pub set_mode: unsafe extern "efiapi" fn(this: &SimpleTextOutput, mode_index: usize) -> usize,
    pub set_attribute: unsafe extern "efiapi" fn(this: &SimpleTextOutput, attr: u8) -> usize,
    pub clear_screen: unsafe extern "efiapi" fn(this: &SimpleTextOutput) -> usize,
    pub set_cursor: unsafe extern "efiapi" fn(
        this: &SimpleTextOutput,
        column: usize,
        row: usize,
    ) -> usize,
    pub mode: *mut c_void,
}

/// EFI_SYSTEM_TABLE。
///
/// 固件以签名 `0x5453595320494249`（"IBI SYST" 小端）校验。
#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *mut Char16,
    pub firmware_revision: u32,
    pub console_in_handle: *mut c_void,
    pub con_in: *mut c_void,
    pub console_out_handle: *mut c_void,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: *mut c_void,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut c_void,
    pub boot_services: *mut c_void,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut c_void,
}

impl SystemTable {
    /// 系统表签名（小端 "IBI SYST"）。
    pub const SIGNATURE: u64 = 0x5453_5953_2049_4249;

    /// 校验签名。不符返回 None，调用方须立即放弃固件服务。
    pub fn from_ptr(ptr: *mut c_void) -> Option<&'static SystemTable> {
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

/// 无堆 Vec：栈上固定容量，超出即截断。引导早期无堆可用。
pub mod alloc_vec {
    pub struct Vec<T, const N: usize> {
        buf: [T; N],
        len: usize,
    }

    impl<T: Copy + Default, const N: usize> Vec<T, N> {
        pub fn new() -> Self {
            Self { buf: [T::default(); N], len: 0 }
        }

        pub fn push(&mut self, item: T) {
            if self.len < N {
                self.buf[self.len] = item;
                self.len += 1;
            }
        }

        pub fn as_slice(&self) -> &[T] {
            &self.buf[..self.len]
        }
    }
}