//! `firmware::graphics::GraphicsSink` 的 UEFI 实现。

use crate::graphics::{GraphicsOutputMode, ModeInformation};
use crate::guid::Guid;
use crate::pixel_format::pixel_format_of;
use arch::addr::PhysAddr;
use firmware::error::Error;
use firmware::graphics::{FramebufferInfo, GraphicsSink};

/// `EFI_GRAPHICS_OUTPUT_PROTOCOL` 的 GUID：`{9042a9de-23dc-4a38-96fb-7aded080516a}`（EDK2 核实）。
pub const GRAPHICS_OUTPUT_PROTOCOL: Guid = Guid {
    data1: 0x9042_a9de,
    data2: 0x23dc,
    data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};

/// 基于 UEFI `GraphicsOutput` 的图形输出。
pub struct UefiGraphics<'a> {
    mode: &'a GraphicsOutputMode,
}

impl<'a> UefiGraphics<'a> {
    /// 以固件给出的当前模式描述构造。
    pub const fn new(mode: &'a GraphicsOutputMode) -> Self {
        Self { mode }
    }
}

impl GraphicsSink for UefiGraphics<'_> {
    fn framebuffer(&self) -> Result<FramebufferInfo, Error> {
        let info = self.mode.info;
        if info.is_null() {
            return Err(Error::Io);
        }
        // SAFETY: `info` 非空，且由固件保证指向有效的模式信息。
        let info: &ModeInformation = unsafe { &*info };
        let format = pixel_format_of(info)?;
        if format.bits_per_pixel % 8 != 0 {
            return Err(Error::Unsupported);
        }
        let bytes_per_pixel = (format.bits_per_pixel / 8) as u64;
        // 数值论证（S19）：`pixels_per_scan_line` 是 u32、每像素 <= 4 字节，
        // 乘积 <= 2^34，在 u64 内不会溢出。
        let pitch = (info.pixels_per_scan_line as u64) * bytes_per_pixel;
        if pitch > u32::MAX as u64 {
            return Err(Error::InvalidArgument);
        }
        Ok(FramebufferInfo {
            base: PhysAddr::new(self.mode.framebuffer_base),
            width: info.horizontal_resolution,
            height: info.vertical_resolution,
            pitch: pitch as u32,
            format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::UefiGraphics;
    use crate::graphics::{GraphicsOutputMode, ModeInformation, PixelBitmask};
    use crate::pixel_format::PIXEL_FORMAT_RGBR8;
    use firmware::error::Error;
    use firmware::graphics::GraphicsSink;

    static INFO: ModeInformation = ModeInformation {
        version: 0,
        horizontal_resolution: 1024,
        vertical_resolution: 768,
        pixel_format: PIXEL_FORMAT_RGBR8,
        pixel_information: PixelBitmask {
            red_mask: 0,
            green_mask: 0,
            blue_mask: 0,
            reserved_mask: 0,
        },
        pixels_per_scan_line: 1024,
    };

    fn mode(info: *mut ModeInformation, base: u64) -> GraphicsOutputMode {
        GraphicsOutputMode {
            max_mode: 1,
            mode: 0,
            info,
            size_of_info: 36,
            framebuffer_base: base,
            framebuffer_size: 1024 * 768 * 4,
        }
    }

    #[test]
    fn a_valid_mode_yields_a_well_formed_framebuffer() {
        let m = mode((&INFO as *const ModeInformation).cast_mut(), 0x8000_0000);
        let sink = UefiGraphics::new(&m);
        let fb = sink.framebuffer().expect("帧缓冲可取");
        assert_eq!(fb.base.as_u64(), 0x8000_0000);
        assert_eq!(fb.width, 1024);
        assert_eq!(fb.height, 768);
        assert_eq!(fb.pitch, 4096, "pitch = 每行像素 × 每像素字节");
        assert_eq!(fb.format.bits_per_pixel, 32);
        assert!(fb.is_valid(), "描述必须自洽");
    }

    #[test]
    fn a_null_mode_info_is_a_firmware_violation() {
        let m = mode(core::ptr::null_mut(), 0);
        let sink = UefiGraphics::new(&m);
        assert_eq!(sink.framebuffer(), Err(Error::Io));
    }
}
