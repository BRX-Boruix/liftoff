//! `EFI_FILE_INFO` 与 `EFI_TIME`。
//!
//! 布局对照 EDK2 `MdePkg/Include/Guid/FileInfo.h` 核实（2026-09-30），并用偏移测试钉住。
//! `FileName` 是柔性数组，故本结构只声明到 `Attribute`（80 字节前缀）。

use crate::file::FileProtocol;
use crate::guid::Guid;
use crate::types::Status;
use core::ffi::c_void;

/// `EFI_TIME`（UEFI 规范）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Time {
    /// 年。
    pub year: u16,
    /// 月。
    pub month: u8,
    /// 日。
    pub day: u8,
    /// 时。
    pub hour: u8,
    /// 分。
    pub minute: u8,
    /// 秒。
    pub second: u8,
    /// 填充。
    pub pad1: u8,
    /// 纳秒。
    pub nanosecond: u32,
    /// 时区。
    pub timezone: i16,
    /// 夏令时标志。
    pub daylight: u8,
    /// 填充。
    pub pad2: u8,
}

/// `EFI_FILE_INFO` 的前缀（到 `Attribute`）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FileInfo {
    /// 结构自身大小（含文件名）。
    pub size: u64,
    /// 文件大小（字节）。
    pub file_size: u64,
    /// 占用的物理空间。
    pub physical_size: u64,
    /// 创建时间。
    pub create_time: Time,
    /// 最后访问时间。
    pub last_access_time: Time,
    /// 最后修改时间。
    pub modification_time: Time,
    /// 属性位。
    pub attribute: u64,
}

/// `EFI_FILE_INFO_ID`：`{09576e92-6d3f-11d2-8e39-00a0c969723b}`（EDK2 核实）。
pub const FILE_INFO_ID: Guid = Guid {
    data1: 0x0957_6e92,
    data2: 0x6d3f,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// 只读属性位（UEFI 规范：`EFI_FILE_READ_ONLY`）。
pub const FILE_ATTRIBUTE_READ_ONLY: u64 = 0x0000_0000_0000_0001;

/// `EFI_FILE_GET_INFO` 的签名。
pub type GetInfo = unsafe extern "efiapi" fn(
    this: *mut FileProtocol,
    information_type: *const Guid,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Status;

#[cfg(test)]
mod tests {
    use super::{FILE_ATTRIBUTE_READ_ONLY, FILE_INFO_ID, FileInfo, Time};
    use core::mem::{offset_of, size_of};

    #[test]
    fn efi_time_layout_matches_the_spec() {
        assert_eq!(size_of::<Time>(), 16);
        assert_eq!(offset_of!(Time, year), 0);
        assert_eq!(offset_of!(Time, month), 2);
        assert_eq!(offset_of!(Time, day), 3);
        assert_eq!(offset_of!(Time, hour), 4);
        assert_eq!(offset_of!(Time, minute), 5);
        assert_eq!(offset_of!(Time, second), 6);
        assert_eq!(offset_of!(Time, nanosecond), 8);
        assert_eq!(offset_of!(Time, timezone), 12);
        assert_eq!(offset_of!(Time, daylight), 14);
    }

    #[test]
    fn file_info_offsets_match_the_spec() {
        assert_eq!(size_of::<FileInfo>(), 80);
        assert_eq!(offset_of!(FileInfo, size), 0);
        assert_eq!(offset_of!(FileInfo, file_size), 8);
        assert_eq!(offset_of!(FileInfo, physical_size), 16);
        assert_eq!(offset_of!(FileInfo, create_time), 24);
        assert_eq!(offset_of!(FileInfo, last_access_time), 40);
        assert_eq!(offset_of!(FileInfo, modification_time), 56);
        assert_eq!(offset_of!(FileInfo, attribute), 72);
    }

    #[test]
    fn file_info_guid_is_stored_little_endian() {
        // EDK2: {0x9576e92, 0x6d3f, 0x11d2, {0x8e,0x39,0x0,0xa0,0xc9,0x69,0x72,0x3b}}
        let raw = unsafe {
            core::slice::from_raw_parts((&FILE_INFO_ID as *const crate::guid::Guid).cast::<u8>(), 16)
        };
        assert_eq!(raw[0], 0x92);
        assert_eq!(raw[3], 0x09);
        assert_eq!(raw[4], 0x3f);
        assert_eq!(raw[5], 0x6d);
        assert_eq!(raw[8], 0x8e);
        assert_eq!(raw[15], 0x3b);
    }

    #[test]
    fn read_only_attribute_bit_is_bit_zero() {
        assert_eq!(FILE_ATTRIBUTE_READ_ONLY, 1);
    }
}
