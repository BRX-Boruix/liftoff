//! `GraphicsOutput` 协议的像素格式与掩码算术。
//!
//! 边界：只做**布局**与**掩码到通道的算术**；像素写入由 `crates/flanterm_rust` 与上层负责。

/// `EFI_PIXEL_BITMASK`（UEFI 规范）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PixelBitmask {
    /// 红色通道掩码。
    pub red_mask: u32,
    /// 绿色通道掩码。
    pub green_mask: u32,
    /// 蓝色通道掩码。
    pub blue_mask: u32,
    /// 保留通道掩码。
    pub reserved_mask: u32,
}

/// `EFI_GRAPHICS_OUTPUT_MODE_INFORMATION`（UEFI 规范）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ModeInformation {
    /// 结构版本。
    pub version: u32,
    /// 水平分辨率（像素）。
    pub horizontal_resolution: u32,
    /// 垂直分辨率（像素）。
    pub vertical_resolution: u32,
    /// 像素格式（0=RGBR8，1=BGR8，2=BitMask，3=BltOnly）。
    pub pixel_format: u32,
    /// 位掩码信息。
    pub pixel_information: PixelBitmask,
    /// 每扫描行像素数（不等于水平分辨率时即为 pitch 填充）。
    pub pixels_per_scan_line: u32,
}

/// `EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE`（UEFI 规范）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GraphicsOutputMode {
    /// 最大模式号。
    pub max_mode: u32,
    /// 当前模式号。
    pub mode: u32,
    /// 当前模式信息。
    pub info: *mut ModeInformation,
    /// 模式信息大小。
    pub size_of_info: usize,
    /// 帧缓冲物理基址。
    pub framebuffer_base: u64,
    /// 帧缓冲大小。
    pub framebuffer_size: usize,
}

/// 掩码的最低置位位序号；掩码为 0 返回 `None`。
pub const fn mask_shift(mask: u32) -> Option<u8> {
    if mask == 0 {
        return None;
    }
    Some(mask.trailing_zeros() as u8)
}

/// 掩码覆盖的位数；掩码为 0 返回 `None`。
///
/// 定义：置位数（popcount）。掩码允许不连续（例如 `0xF0` 覆盖 4 位）。
pub const fn mask_bits(mask: u32) -> Option<u8> {
    if mask == 0 {
        return None;
    }
    Some(mask.count_ones() as u8)
}

#[cfg(test)]
mod tests {
    use super::{GraphicsOutputMode, ModeInformation, mask_bits, mask_shift};
    use core::mem::offset_of;

    #[test]
    fn mask_shift_is_the_lowest_set_bit() {
        assert_eq!(mask_shift(0xFF), Some(0));
        assert_eq!(mask_shift(0xFF_0000), Some(16));
        assert_eq!(mask_shift(1 << 31), Some(31));
        assert_eq!(mask_shift(0), None);
    }

    #[test]
    fn mask_bits_counts_the_covered_bits() {
        assert_eq!(mask_bits(0x1), Some(1));
        assert_eq!(mask_bits(0xFF), Some(8));
        assert_eq!(mask_bits(0xFF_0000), Some(8));
        assert_eq!(mask_bits(0xF0), Some(4));
        assert_eq!(mask_bits(0), None);
    }

    #[test]
    fn mode_information_layout_matches_the_spec() {
        assert_eq!(offset_of!(ModeInformation, version), 0);
        assert_eq!(offset_of!(ModeInformation, horizontal_resolution), 4);
        assert_eq!(offset_of!(ModeInformation, vertical_resolution), 8);
        assert_eq!(offset_of!(ModeInformation, pixel_format), 12);
        assert_eq!(offset_of!(ModeInformation, pixel_information), 16);
        assert_eq!(offset_of!(ModeInformation, pixels_per_scan_line), 32);
    }

    #[test]
    fn graphics_output_mode_layout_matches_the_spec() {
        assert_eq!(offset_of!(GraphicsOutputMode, max_mode), 0);
        assert_eq!(offset_of!(GraphicsOutputMode, mode), 4);
        assert_eq!(offset_of!(GraphicsOutputMode, info), 8);
        assert_eq!(offset_of!(GraphicsOutputMode, size_of_info), 16);
        assert_eq!(offset_of!(GraphicsOutputMode, framebuffer_base), 24);
        assert_eq!(offset_of!(GraphicsOutputMode, framebuffer_size), 32);
    }
}
