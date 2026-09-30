//! Limine 引导器信息协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应是 { u64 revision; char *name; char *version; }，共 24 字节。

use crate::base::COMMON_MAGIC;

/// `LIMINE_BOOTLOADER_INFO_REQUEST_ID`。
pub const BOOTLOADER_INFO_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0xf55038d8e2a1202f,
    0x279426fcf5f59740,
];

/// `struct limine_bootloader_info_response`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootloaderInfoResponse {
    /// 响应修订。
    pub revision: u64,
    /// 引导器名称。
    pub name: *mut core::ffi::c_char,
    /// 引导器版本。
    pub version: *mut core::ffi::c_char,
}

/// `struct limine_bootloader_info_request`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootloaderInfoRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut BootloaderInfoResponse,
}

#[cfg(test)]
mod tests {
    use super::{
        BOOTLOADER_INFO_REQUEST_ID, BootloaderInfoRequest, BootloaderInfoResponse,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<BootloaderInfoResponse>(), 24);
        assert_eq!(offset_of!(BootloaderInfoResponse, revision), 0);
        assert_eq!(offset_of!(BootloaderInfoResponse, name), 8);
        assert_eq!(offset_of!(BootloaderInfoResponse, version), 16);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<BootloaderInfoRequest>(), 48);
        assert_eq!(offset_of!(BootloaderInfoRequest, response), 40);
    }

    #[test]
    fn request_id_matches_the_header() {
        assert_eq!(BOOTLOADER_INFO_REQUEST_ID[2], 0xf55038d8e2a1202f);
        assert_eq!(BOOTLOADER_INFO_REQUEST_ID[3], 0x279426fcf5f59740);
    }
}
