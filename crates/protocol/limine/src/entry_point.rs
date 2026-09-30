//! Limine 入口点协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应是 { u64 revision; }（8 字节，最小的响应）；
//! 请求是标准头 + 一个函数指针字段 entry，共 56 字节。

use crate::base::COMMON_MAGIC;

/// `LIMINE_ENTRY_POINT_REQUEST_ID`。
pub const ENTRY_POINT_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x13d86c035a1cd3e1,
    0x2b0caa89d8f3026a,
];

/// `limine_entry_point`：内核入口。
pub type EntryPoint = unsafe extern "C" fn();

/// `struct limine_entry_point_response`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EntryPointResponse {
    /// 响应修订。
    pub revision: u64,
}

/// `struct limine_entry_point_request`（比标准头多一个 entry 字段）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EntryPointRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut EntryPointResponse,
    /// 内核入口（内核填写；空表示未设置）。
    pub entry: Option<EntryPoint>,
}

#[cfg(test)]
mod tests {
    use super::{ENTRY_POINT_REQUEST_ID, EntryPointRequest, EntryPointResponse};
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<EntryPointResponse>(), 8);
        assert_eq!(offset_of!(EntryPointResponse, revision), 0);
    }

    #[test]
    fn request_has_an_extra_entry_field() {
        assert_eq!(size_of::<EntryPointRequest>(), 56);
        assert_eq!(offset_of!(EntryPointRequest, response), 40);
        assert_eq!(offset_of!(EntryPointRequest, entry), 48);
    }

    #[test]
    fn request_id_matches_the_header() {
        assert_eq!(ENTRY_POINT_REQUEST_ID[2], 0x13d86c035a1cd3e1);
        assert_eq!(ENTRY_POINT_REQUEST_ID[3], 0x2b0caa89d8f3026a);
    }
}
