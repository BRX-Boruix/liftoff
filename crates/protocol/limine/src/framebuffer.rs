//! Limine framebuffer protocol (request/response pair).
//!
//! Verified against brxLimine/limine-protocol/include/limine.h (2026-09-30).
//! The per-framebuffer structure (`limine_framebuffer`) is deliberately NOT declared yet:
//! its head fields have not been verified, so `framebuffers` stays an opaque double pointer.

use crate::base::COMMON_MAGIC;

/// `LIMINE_FRAMEBUFFER_REQUEST_ID`.
pub const FRAMEBUFFER_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x9d5827dcd881dd75,
    0xa3148604f6fab11b,
];

/// `LIMINE_FRAMEBUFFER_RGB` (memory model).
pub const FRAMEBUFFER_RGB: u64 = 1;

/// `struct limine_framebuffer_response`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FramebufferResponse {
    /// Response revision.
    pub revision: u64,
    /// Number of framebuffers.
    pub framebuffer_count: u64,
    /// Pointer to an array of framebuffer pointers.
    pub framebuffers: *mut *mut core::ffi::c_void,
}

/// `struct limine_framebuffer_request`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FramebufferRequest {
    /// Request identifier.
    pub id: [u64; 4],
    /// Request revision.
    pub revision: u64,
    /// Response pointer (filled by the bootloader).
    pub response: *mut FramebufferResponse,
}

#[cfg(test)]
mod tests {
    use super::{
        FRAMEBUFFER_REQUEST_ID, FRAMEBUFFER_RGB, FramebufferRequest, FramebufferResponse,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<FramebufferResponse>(), 24);
        assert_eq!(offset_of!(FramebufferResponse, revision), 0);
        assert_eq!(offset_of!(FramebufferResponse, framebuffer_count), 8);
        assert_eq!(offset_of!(FramebufferResponse, framebuffers), 16);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<FramebufferRequest>(), 48);
        assert_eq!(offset_of!(FramebufferRequest, response), 40);
    }

    #[test]
    fn request_id_and_rgb_model_match_the_header() {
        assert_eq!(FRAMEBUFFER_REQUEST_ID[2], 0x9d5827dcd881dd75);
        assert_eq!(FRAMEBUFFER_REQUEST_ID[3], 0xa3148604f6fab11b);
        assert_eq!(FRAMEBUFFER_RGB, 1);
    }
}
