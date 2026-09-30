//! UEFI 基础类型与布局。
//!
//! 布局按 UEFI 规范（`EFI_TABLE_HEADER` / `EFI_SYSTEM_TABLE`）定义；偏移由宿主单测断言，
//! 字段增删或顺序变化会让测试立刻失败，而不是在运行期读到错位数据。

use core::ffi::c_void;

/// 固件对象句柄（不透明）。
pub type Handle = *mut c_void;

/// 固件调用返回码。
///
/// 用 newtype 而非裸 `usize`：`is_error()` 才有归属，且避免与普通整数混淆。
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Status(pub usize);

impl Status {
    /// 是否表示错误。
    ///
    /// UEFI 约定：最高位为 1 表示错误；这里按**本机位宽**判断，32/64 位 UEFI 都成立。
    pub const fn is_error(self) -> bool {
        self.0 & (1usize << (usize::BITS - 1)) != 0
    }

    /// 原始数值（与固件 ABI 直接对应）。
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

/// 成功（`EFI_SUCCESS`）。
pub const SUCCESS: Status = Status(0);
/// 参数非法（`EFI_INVALID_PARAMETER`，UEFI 规范附录 D）。
pub const INVALID_PARAMETER: Status = Status(0x8000_0000_0000_0002);
/// 不支持（`EFI_UNSUPPORTED`）。
pub const UNSUPPORTED: Status = Status(0x8000_0000_0000_0003);
/// 缓冲过小（`EFI_BUFFER_TOO_SMALL`）。
pub const BUFFER_TOO_SMALL: Status = Status(0x8000_0000_0000_0005);
/// 设备错误（`EFI_DEVICE_ERROR`）。
pub const DEVICE_ERROR: Status = Status(0x8000_0000_0000_0007);
/// 未找到（`EFI_NOT_FOUND`）。
pub const NOT_FOUND: Status = Status(0x8000_0000_0000_000E);

/// 固件表头（`EFI_TABLE_HEADER`）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TableHeader {
    /// 表签名。
    pub signature: u64,
    /// 规范版本。
    pub revision: u32,
    /// 表头大小（含自身）。
    pub header_size: u32,
    /// 表 CRC32。
    pub crc32: u32,
    /// 保留。
    pub reserved: u32,
}

/// 系统表（`EFI_SYSTEM_TABLE`）。
///
/// 只声明到引导阶段需要使用的字段；未使用的服务表指针保持不透明。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SystemTable {
    /// 表头。
    pub hdr: TableHeader,
    /// 固件厂商字符串。
    pub firmware_vendor: *mut u16,
    /// 固件版本。
    pub firmware_revision: u32,
    /// 标准输入句柄。
    pub console_in_handle: Handle,
    /// 标准输入协议。
    pub console_in: *mut c_void,
    /// 标准输出句柄。
    pub console_out_handle: Handle,
    /// 标准输出协议。
    pub console_out: *mut c_void,
    /// 标准错误句柄。
    pub std_err_handle: Handle,
    /// 标准错误协议。
    pub std_err: *mut c_void,
    /// 运行期服务。
    pub runtime_services: *mut c_void,
    /// 引导服务。
    pub boot_services: *mut c_void,
    /// 配置表项数。
    pub number_of_table_entries: usize,
    /// 配置表。
    pub configuration_table: *mut c_void,
}

#[cfg(test)]
mod tests {
    use super::{Handle, NOT_FOUND, SUCCESS, SystemTable, TableHeader};
    use core::mem::{offset_of, size_of};

    #[test]
    fn status_uses_the_high_bit_convention() {
        assert!(!SUCCESS.is_error());
        assert!(NOT_FOUND.is_error());
        assert_eq!(SUCCESS.as_usize(), 0);
    }

    #[test]
    fn table_header_layout_matches_the_spec() {
        assert_eq!(size_of::<TableHeader>(), 24);
        assert_eq!(offset_of!(TableHeader, signature), 0);
        assert_eq!(offset_of!(TableHeader, revision), 8);
        assert_eq!(offset_of!(TableHeader, header_size), 12);
        assert_eq!(offset_of!(TableHeader, crc32), 16);
        assert_eq!(offset_of!(TableHeader, reserved), 20);
    }

    #[test]
    fn system_table_offsets_match_the_spec() {
        assert_eq!(offset_of!(SystemTable, hdr), 0);
        assert_eq!(offset_of!(SystemTable, firmware_vendor), 24);
        assert_eq!(offset_of!(SystemTable, firmware_revision), 32);
        assert_eq!(offset_of!(SystemTable, console_in_handle), 40);
        assert_eq!(offset_of!(SystemTable, boot_services), 96);
        assert_eq!(offset_of!(SystemTable, configuration_table), 112);
    }

    #[test]
    fn handle_is_pointer_sized() {
        assert_eq!(size_of::<Handle>(), size_of::<*mut core::ffi::c_void>());
    }
}
