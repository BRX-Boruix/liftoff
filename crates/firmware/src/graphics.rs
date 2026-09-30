//! 图形输出：固件提供的线性帧缓冲。
//!
//! 边界：只描述“线性帧缓冲 + 像素格式”并暴露取用接口；绘制（文本/图形）归
//! `crates/flanterm_rust` 与上层，不在本层出现。

use crate::error::Error;
use arch::addr::PhysAddr;

/// 像素格式：每像素位数与通道位偏移。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelFormat {
    /// 每像素位数（支持 8/16/24/32）。
    pub bits_per_pixel: u16,
    /// 红色通道位偏移。
    pub red_shift: u8,
    /// 绿色通道位偏移。
    pub green_shift: u8,
    /// 蓝色通道位偏移。
    pub blue_shift: u8,
    /// 红通道掩码宽度（位）。
    pub red_mask_size: u8,
    /// 绿通道掩码宽度（位）。
    pub green_mask_size: u8,
    /// 蓝通道掩码宽度（位）。
    pub blue_mask_size: u8,
}

/// 线性帧缓冲描述。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FramebufferInfo {
    /// 物理基址。
    pub base: PhysAddr,
    /// 宽度（像素）。
    pub width: u32,
    /// 高度（像素）。
    pub height: u32,
    /// 每行字节数（pitch）。
    pub pitch: u32,
    /// 像素格式。
    pub format: PixelFormat,
}

impl FramebufferInfo {
    /// 缓冲总字节数。
    ///
    /// 数值论证（S19）：`pitch` 与 `height` 均为 `u32`，乘积 `<= (2^32-1)^2 < 2^64`，
    /// 在 `u64` 内**不会溢出**；仍保留 `checked_mul` 作为防御性写法。
    pub const fn byte_len(self) -> Option<u64> {
        (self.pitch as u64).checked_mul(self.height as u64)
    }

    /// 描述是否自洽。
    ///
    /// 数值论证（S19）：`width` 为 `u32` 且 `bpp <= 32`，故 `width * bpp <= 2^37`，
    /// 在 `u64` 内不会溢出，可用无 checked 的乘法。
    pub const fn is_valid(self) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let bpp = self.format.bits_per_pixel as u64;
        if !matches!(bpp, 8 | 16 | 24 | 32) {
            return false;
        }
        let min_pitch = (self.width as u64 * bpp + 7) / 8;
        if (self.pitch as u64) < min_pitch {
            return false;
        }
        let bpp8 = self.format.bits_per_pixel as u8;
        self.format.red_shift < bpp8
            && self.format.green_shift < bpp8
            && self.format.blue_shift < bpp8
    }
}

/// 图形输出：固件层能力 trait 之一。
pub trait GraphicsSink {
    /// 取线性帧缓冲描述；无图形输出时返回 `Error::Unsupported`。
    fn framebuffer(&self) -> Result<FramebufferInfo, Error>;
}

#[cfg(test)]
mod tests {
    use super::{FramebufferInfo, PixelFormat};
    use arch::addr::PhysAddr;

    fn fb(width: u32, height: u32, pitch: u32, bpp: u16, red: u8, green: u8, blue: u8) -> FramebufferInfo {
        FramebufferInfo {
            base: PhysAddr::new(0x8000_0000),
            width,
            height,
            pitch,
            format: PixelFormat {
                bits_per_pixel: bpp,
                red_shift: red,
                green_shift: green,
                blue_shift: blue,
                red_mask_size: 8,
                green_mask_size: 8,
                blue_mask_size: 8,
            },
        }
    }

    #[test]
    fn a_typical_32bpp_framebuffer_is_valid() {
        assert!(fb(1024, 768, 4096, 32, 16, 8, 0).is_valid());
        assert_eq!(fb(1024, 768, 4096, 32, 16, 8, 0).byte_len(), Some(4096 * 768));
    }

    #[test]
    fn zero_size_and_bad_pitch_are_rejected() {
        assert!(!fb(0, 768, 4096, 32, 16, 8, 0).is_valid());
        assert!(!fb(1024, 0, 4096, 32, 16, 8, 0).is_valid());
        assert!(!fb(1024, 768, 4095, 32, 16, 8, 0).is_valid());
    }

    #[test]
    fn unsupported_pixel_widths_are_rejected() {
        assert!(!fb(1024, 768, 4096, 12, 16, 8, 0).is_valid());
        assert!(!fb(1024, 768, 4096, 0, 16, 8, 0).is_valid());
    }

    #[test]
    fn channel_shifts_must_fit_the_pixel_width() {
        assert!(!fb(1024, 768, 4096, 8, 8, 0, 0).is_valid());
    }

    #[test]
    fn byte_len_never_overflows_for_u32_inputs() {
        // 数值论证（S19）：pitch 与 height 均为 u32，乘积 <= (2^32-1)^2 < 2^64。
        assert!(fb(1, u32::MAX, u32::MAX, 32, 16, 8, 0).byte_len().is_some());
    }
}