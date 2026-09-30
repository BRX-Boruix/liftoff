//! Limine RSDP 协议（请求/响应对）。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应是 { u64 revision; void *address; }，共 16 字节。

use crate::base::COMMON_MAGIC;

/// `LIMINE_RSDP_REQUEST_ID`.
pub const RSDP_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0xc5e77b6b397e7b43,
    0x27637845accdcf3c,
];

/// `struct limine_rsdp_response`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RsdpResponse {
    /// 响应修订。
    pub revision: u64,
    /// RSDP 的地址（内核据此找 ACPI 表）。
    pub address: *mut core::ffi::c_void,
}

/// `struct limine_rsdp_request`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RsdpRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut RsdpResponse,
}

#[cfg(test)]
mod tests {
    use super::{RSDP_REQUEST_ID, RsdpRequest, RsdpResponse};
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<RsdpResponse>(), 16);
        assert_eq!(offset_of!(RsdpResponse, revision), 0);
        assert_eq!(offset_of!(RsdpResponse, address), 8);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<RsdpRequest>(), 48);
        assert_eq!(offset_of!(RsdpRequest, response), 40);
    }

    #[test]
    fn request_id_matches_the_header() {
        assert_eq!(RSDP_REQUEST_ID[2], 0xc5e77b6b397e7b43);
        assert_eq!(RSDP_REQUEST_ID[3], 0x27637845accdcf3c);
    }
}
