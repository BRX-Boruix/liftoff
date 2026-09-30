//! Limine 帧缓冲协议（请求/响应对）。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）。
//! 帧缓冲结构体 limine_framebuffer 的头部字段已核实（2026-09-30），故已声明。

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

/// 帧缓冲内存模型：RGB（limine.h 的 `LIMINE_FRAMEBUFFER_RGB`）。
pub const MEMORY_MODEL_RGB: u8 = 1;

/// `struct limine_framebuffer`.
///
/// 头部字段逐一对照 limine.h 核实（2026-09-30）：address/width/height/pitch/bpp/
/// memory_model/各通道掩码尺寸与位移/unused[7]/edid_size/edid/mode_count/modes。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Framebuffer {
    /// 线性帧缓冲的虚拟地址（引导器映射后可直接写）。
    pub address: *mut core::ffi::c_void,
    /// 宽度（像素）。
    pub width: u64,
    /// 高度（像素）。
    pub height: u64,
    /// 每行字节数。
    pub pitch: u64,
    /// 每像素位数。
    pub bpp: u16,
    /// 内存模型（1 = RGB）。
    pub memory_model: u8,
    /// 红色通道掩码位数。
    pub red_mask_size: u8,
    /// 红色通道位移。
    pub red_mask_shift: u8,
    /// 绿色通道掩码位数。
    pub green_mask_size: u8,
    /// 绿色通道位移。
    pub green_mask_shift: u8,
    /// 蓝色通道掩码位数。
    pub blue_mask_size: u8,
    /// 蓝色通道位移。
    pub blue_mask_shift: u8,
    /// 保留（对齐用）。
    pub unused: [u8; 7],
    /// EDID 大小。
    pub edid_size: u64,
    /// EDID 指针。
    pub edid: *mut core::ffi::c_void,
    /// 视频模式数量（响应修订 1 起）。
    pub mode_count: u64,
    /// 视频模式数组（双重指针）。
    pub modes: *mut *mut core::ffi::c_void,
}

impl Framebuffer {
    /// 占位值（便于调用方初始化数组）。
    pub const EMPTY: Self = Self {
        address: core::ptr::null_mut(),
        width: 0,
        height: 0,
        pitch: 0,
        bpp: 0,
        memory_model: 0,
        red_mask_size: 0,
        red_mask_shift: 0,
        green_mask_size: 0,
        green_mask_shift: 0,
        blue_mask_size: 0,
        blue_mask_shift: 0,
        unused: [0; 7],
        edid_size: 0,
        edid: core::ptr::null_mut(),
        mode_count: 0,
        modes: core::ptr::null_mut(),
    };
}

/// `struct limine_framebuffer_response`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FramebufferResponse {
    /// 响应修订。
    pub revision: u64,
    /// 帧缓冲数量。
    pub framebuffer_count: u64,
    /// 指向“帧缓冲指针数组”的指针。
    pub framebuffers: *mut *mut Framebuffer,
}

/// `struct limine_framebuffer_request`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FramebufferRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut FramebufferResponse,
}

#[cfg(test)]
mod tests {
    use super::{FRAMEBUFFER_REQUEST_ID, FRAMEBUFFER_RGB, Framebuffer, FramebufferRequest, FramebufferResponse};
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<FramebufferResponse>(), 24);
        assert_eq!(offset_of!(FramebufferResponse, revision), 0);
        assert_eq!(offset_of!(FramebufferResponse, framebuffer_count), 8);
        assert_eq!(offset_of!(FramebufferResponse, framebuffers), 16);
    }

    #[test]
    fn framebuffer_layout_matches_the_header() {
        assert_eq!(size_of::<Framebuffer>(), 80);
        assert_eq!(offset_of!(Framebuffer, address), 0);
        assert_eq!(offset_of!(Framebuffer, width), 8);
        assert_eq!(offset_of!(Framebuffer, height), 16);
        assert_eq!(offset_of!(Framebuffer, pitch), 24);
        assert_eq!(offset_of!(Framebuffer, bpp), 32);
        assert_eq!(offset_of!(Framebuffer, memory_model), 34);
        assert_eq!(offset_of!(Framebuffer, red_mask_size), 35);
        assert_eq!(offset_of!(Framebuffer, red_mask_shift), 36);
        assert_eq!(offset_of!(Framebuffer, green_mask_size), 37);
        assert_eq!(offset_of!(Framebuffer, green_mask_shift), 38);
        assert_eq!(offset_of!(Framebuffer, blue_mask_size), 39);
        assert_eq!(offset_of!(Framebuffer, blue_mask_shift), 40);
        assert_eq!(offset_of!(Framebuffer, unused), 41);
        assert_eq!(offset_of!(Framebuffer, edid_size), 48);
        assert_eq!(offset_of!(Framebuffer, edid), 56);
        assert_eq!(offset_of!(Framebuffer, mode_count), 64);
        assert_eq!(offset_of!(Framebuffer, modes), 72);
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