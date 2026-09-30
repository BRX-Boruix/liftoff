//! Limine 固件类型协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应是 { u64 revision; u64 firmware_type; }，共 16 字节；类型共 4 个。

use crate::base::COMMON_MAGIC;

/// `LIMINE_FIRMWARE_TYPE_REQUEST_ID`。
pub const FIRMWARE_TYPE_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x8c2f75d90bef28a8,
    0x7045a4688eac00c3,
];

/// `LIMINE_FIRMWARE_TYPE_X86BIOS`。
pub const X86BIOS: u64 = 0;

/// `LIMINE_FIRMWARE_TYPE_EFI32`。
pub const EFI32: u64 = 1;

/// `LIMINE_FIRMWARE_TYPE_EFI64`。
pub const EFI64: u64 = 2;

/// `LIMINE_FIRMWARE_TYPE_SBI`。
pub const SBI: u64 = 3;

/// `struct limine_firmware_type_response`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FirmwareTypeResponse {
    /// 响应修订。
    pub revision: u64,
    /// 固件类型（见 `X86BIOS`/`EFI32`/`EFI64`/`SBI`）。
    pub firmware_type: u64,
}

/// `struct limine_firmware_type_request`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FirmwareTypeRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut FirmwareTypeResponse,
}

#[cfg(test)]
mod tests {
    use super::{
        EFI32, EFI64, FIRMWARE_TYPE_REQUEST_ID, FirmwareTypeRequest, FirmwareTypeResponse, SBI,
        X86BIOS,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<FirmwareTypeResponse>(), 16);
        assert_eq!(offset_of!(FirmwareTypeResponse, revision), 0);
        assert_eq!(offset_of!(FirmwareTypeResponse, firmware_type), 8);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<FirmwareTypeRequest>(), 48);
        assert_eq!(offset_of!(FirmwareTypeRequest, response), 40);
    }

    #[test]
    fn id_and_type_values_match_the_header() {
        assert_eq!(FIRMWARE_TYPE_REQUEST_ID[2], 0x8c2f75d90bef28a8);
        assert_eq!(FIRMWARE_TYPE_REQUEST_ID[3], 0x7045a4688eac00c3);
        assert_eq!(X86BIOS, 0);
        assert_eq!(EFI32, 1);
        assert_eq!(EFI64, 2);
        assert_eq!(SBI, 3);
    }
}
