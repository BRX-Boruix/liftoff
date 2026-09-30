//! Limine 可执行文件地址协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应是 { u64 revision; u64 physical_base; u64 virtual_base; }，共 24 字节。

use crate::base::COMMON_MAGIC;

/// `LIMINE_EXECUTABLE_ADDRESS_REQUEST_ID`。
pub const EXECUTABLE_ADDRESS_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x71ba76863cc55f63,
    0xb2644a48c516a487,
];

/// `struct limine_executable_address_response`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExecutableAddressResponse {
    /// 响应修订。
    pub revision: u64,
    /// 内核被装载到的物理基址。
    pub physical_base: u64,
    /// 内核的虚拟基址。
    pub virtual_base: u64,
}

/// `struct limine_executable_address_request`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExecutableAddressRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut ExecutableAddressResponse,
}

#[cfg(test)]
mod tests {
    use super::{
        EXECUTABLE_ADDRESS_REQUEST_ID, ExecutableAddressRequest, ExecutableAddressResponse,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<ExecutableAddressResponse>(), 24);
        assert_eq!(offset_of!(ExecutableAddressResponse, revision), 0);
        assert_eq!(offset_of!(ExecutableAddressResponse, physical_base), 8);
        assert_eq!(offset_of!(ExecutableAddressResponse, virtual_base), 16);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<ExecutableAddressRequest>(), 48);
        assert_eq!(offset_of!(ExecutableAddressRequest, response), 40);
    }

    #[test]
    fn request_id_matches_the_header() {
        assert_eq!(EXECUTABLE_ADDRESS_REQUEST_ID[2], 0x71ba76863cc55f63);
        assert_eq!(EXECUTABLE_ADDRESS_REQUEST_ID[3], 0xb2644a48c516a487);
    }
}
