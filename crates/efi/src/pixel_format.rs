//! UEFI 像素格式 → 抽象像素格式。
//!
//! 边界：只做**格式换算**；帧缓冲描述由 `GraphicsSink` 实现组装。

use crate::graphics::{ModeInformation, mask_bits, mask_shift};
use firmware::error::Error;
use firmware::graphics::PixelFormat;

/// 像素格式码：`PixelRedGreenBlueReserved8BitPerColor`。
pub const PIXEL_FORMAT_RGBR8: u32 = 0;
/// 像素格式码：`PixelBlueGreenRedReserved8BitPerColor`。
pub const PIXEL_FORMAT_BGR8: u32 = 1;
/// 像素格式码：`PixelBitMask`。
pub const PIXEL_FORMAT_BITMASK: u32 = 2;
/// 像素格式码：`PixelBltOnly`（只能经 Blt 操作，不能直接写像素）。
pub const PIXEL_FORMAT_BLT_ONLY: u32 = 3;

/// 由模式信息推出抽象像素格式。
///
/// `BitMask` 模式下：通道位移取各掩码的最低置位位序号，像素宽度取四个掩码位宽之和
/// （含保留通道）。`BltOnly` 与未知格式返回 `Error::Unsupported`（不能直接写像素）。
pub fn pixel_format_of(mode: &ModeInformation) -> Result<PixelFormat, Error> {
    match mode.pixel_format {
        PIXEL_FORMAT_RGBR8 => Ok(PixelFormat {
            bits_per_pixel: 32,
            red_shift: 0,
            green_shift: 8,
            blue_shift: 16,
        }),
        PIXEL_FORMAT_BGR8 => Ok(PixelFormat {
            bits_per_pixel: 32,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }),
        PIXEL_FORMAT_BITMASK => {
            let masks = &mode.pixel_information;
            let red_shift = mask_shift(masks.red_mask).ok_or(Error::Unsupported)?;
            let green_shift = mask_shift(masks.green_mask).ok_or(Error::Unsupported)?;
            let blue_shift = mask_shift(masks.blue_mask).ok_or(Error::Unsupported)?;
            let red_bits = mask_bits(masks.red_mask).ok_or(Error::Unsupported)? as u16;
            let green_bits = mask_bits(masks.green_mask).ok_or(Error::Unsupported)? as u16;
            let blue_bits = mask_bits(masks.blue_mask).ok_or(Error::Unsupported)? as u16;
            let reserved_bits = mask_bits(masks.reserved_mask).unwrap_or(0) as u16;
            Ok(PixelFormat {
                bits_per_pixel: red_bits + green_bits + blue_bits + reserved_bits,
                red_shift,
                green_shift,
                blue_shift,
            })
        }
        _ => Err(Error::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PIXEL_FORMAT_BGR8, PIXEL_FORMAT_BITMASK, PIXEL_FORMAT_BLT_ONLY, PIXEL_FORMAT_RGBR8,
        pixel_format_of,
    };
    use crate::graphics::{ModeInformation, PixelBitmask};
    use firmware::error::Error;

    fn mode(pixel_format: u32, masks: [u32; 4]) -> ModeInformation {
        ModeInformation {
            version: 0,
            horizontal_resolution: 1024,
            vertical_resolution: 768,
            pixel_format,
            pixel_information: PixelBitmask {
                red_mask: masks[0],
                green_mask: masks[1],
                blue_mask: masks[2],
                reserved_mask: masks[3],
            },
            pixels_per_scan_line: 1024,
        }
    }

    #[test]
    fn rgb_and_bgr_have_fixed_channel_layouts() {
        let rgb = pixel_format_of(&mode(PIXEL_FORMAT_RGBR8, [0; 4])).expect("RGBR8");
        assert_eq!((rgb.bits_per_pixel, rgb.red_shift, rgb.green_shift, rgb.blue_shift), (32, 0, 8, 16));
        let bgr = pixel_format_of(&mode(PIXEL_FORMAT_BGR8, [0; 4])).expect("BGR8");
        assert_eq!((bgr.bits_per_pixel, bgr.red_shift, bgr.green_shift, bgr.blue_shift), (32, 16, 8, 0));
    }

    #[test]
    fn bitmask_mode_derives_shifts_and_width_from_the_masks() {
        let fmt = pixel_format_of(&mode(PIXEL_FORMAT_BITMASK, [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0]))
            .expect("BitMask");
        assert_eq!((fmt.red_shift, fmt.green_shift, fmt.blue_shift), (16, 8, 0));
        assert_eq!(fmt.bits_per_pixel, 24, "通道位宽之和即像素宽度");
    }

    #[test]
    fn blt_only_and_unknown_formats_are_unsupported() {
        assert_eq!(pixel_format_of(&mode(PIXEL_FORMAT_BLT_ONLY, [0; 4])), Err(Error::Unsupported));
        assert_eq!(pixel_format_of(&mode(9, [0; 4])), Err(Error::Unsupported));
        assert_eq!(
            pixel_format_of(&mode(PIXEL_FORMAT_BITMASK, [0, 0, 0, 0])),
            Err(Error::Unsupported),
            "零掩码无从推导"
        );
    }
}
