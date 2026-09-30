//! 协议基础类型：魔数、基础修订与第一对请求/响应。
//!
//! 每个常量与偏移都对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30），不凭记忆书写。

/// `LIMINE_COMMON_MAGIC` (the first two words of every request id).
pub const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// `LIMINE_BASE_REVISION(N)` 的魔数。
pub const BASE_REVISION_MAGIC: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

/// 本引导器实现的基础修订。
pub const BASE_REVISION: u64 = 0;

/// `LIMINE_BASE_REVISION_SUPPORTED(VAR)`：内核声明的基础修订数组，
/// 当第三个元素为零时视为受支持。
pub const fn base_revision_supported(declared: [u64; 3]) -> bool {
    declared[2] == 0
}

/// `LIMINE_HHDM_REQUEST_ID`.
pub const HHDM_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x48dcf1cb8ad2b852,
    0x63984e959a98244b,
];

/// `struct limine_hhdm_response`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HhdmResponse {
    /// 响应修订。
    pub revision: u64,
    /// 高半区直接映射（HHDM）偏移。
    pub offset: u64,
}

/// `struct limine_hhdm_request`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HhdmRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut HhdmResponse,
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_REVISION, HHDM_REQUEST_ID, HhdmRequest, HhdmResponse, base_revision_supported,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn base_revision_supported_matches_the_header_macro() {
        // limine.h: LIMINE_BASE_REVISION_SUPPORTED(VAR) is ((VAR)[2] == 0).
        assert!(base_revision_supported([1, 2, BASE_REVISION]));
        assert!(!base_revision_supported([1, 2, 1]));
    }

    #[test]
    fn request_layout_matches_the_header() {
        // limine.h: struct limine_hhdm_request { uint64_t id[4]; uint64_t revision; ptr response; }
        assert_eq!(size_of::<HhdmRequest>(), 48);
        assert_eq!(offset_of!(HhdmRequest, id), 0);
        assert_eq!(offset_of!(HhdmRequest, revision), 32);
        assert_eq!(offset_of!(HhdmRequest, response), 40);
    }

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<HhdmResponse>(), 16);
        assert_eq!(offset_of!(HhdmResponse, revision), 0);
        assert_eq!(offset_of!(HhdmResponse, offset), 8);
    }

    #[test]
    fn hhdm_request_id_matches_the_header() {
        assert_eq!(HHDM_REQUEST_ID[0], 0xc7b1dd30df4c8b88);
        assert_eq!(HHDM_REQUEST_ID[1], 0x0a82e883a194f07b);
        assert_eq!(HHDM_REQUEST_ID[2], 0x48dcf1cb8ad2b852);
        assert_eq!(HHDM_REQUEST_ID[3], 0x63984e959a98244b);
    }
}
