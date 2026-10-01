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

/// UEFI 图形输出协议（`EFI_GRAPHICS_OUTPUT_PROTOCOL`）。
///
/// 只声明我们真正用到的成员：`mode`（当前模式）。其余成员是协议要求的位置占位 ——
/// **我们从不构造它**，它由固件给出，故这里用 `*mut c_void` 而不假装它们是可调用的函数。
#[repr(C)]
pub struct GraphicsOutput {
    /// `QueryMode`（未使用）。
    pub query_mode: *mut core::ffi::c_void,
    /// `SetMode`（未使用）。
    pub set_mode: *mut core::ffi::c_void,
    /// `Blt`（未使用）。
    pub blt: *mut core::ffi::c_void,
    /// 当前模式。
    pub mode: *mut GraphicsOutputMode,
}

/// `EFI_GRAPHICS_OUTPUT_PROTOCOL` 的 GUID。
const GOP_GUID: crate::guid::Guid = crate::guid::Guid {
    data1: 0x9042_a9de,
    data2: 0x23dc,
    data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};

/// 通过固件找到**当前图形模式**；没有 GOP 就返回 `None` —— 不编造帧缓冲。
///
/// 句柄表放在本模块的静态里（入口不该认识 GOP 协议类型）。
///
/// # Safety
///
/// 引导阶段单线程调用一次。
pub unsafe fn graphics_output_mode(
    locate_handle: crate::boot_services_table::LocateHandle,
    handle_protocol: crate::boot_services_table::HandleProtocol,
) -> Option<&'static GraphicsOutputMode> {
    const MAX: usize = 4;
    static mut HANDLES: [crate::types::Handle; MAX] = [core::ptr::null_mut(); MAX];
    let guid = &GOP_GUID as *const crate::guid::Guid as *const core::ffi::c_void;
    // SAFETY: 由调用方保证引导阶段单线程调用一次。
    let handles = unsafe { &mut *core::ptr::addr_of_mut!(HANDLES) };
    let count = crate::enumerate::enumerate_handles(
        locate_handle,
        crate::discover::SEARCH_TYPE_BY_PROTOCOL,
        guid,
        handles,
    )
    .ok()?;
    if count == 0 {
        return None;
    }
    let mut interface: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: `handles[0]` 来自枚举；`interface` 指向本栈上的有效变量。
    let status = unsafe { handle_protocol(handles[0], guid, &mut interface) };
    if crate::status::status_to_error(status).is_some() || interface.is_null() {
        return None;
    }
    let output = interface as *const GraphicsOutput;
    // SAFETY: 协议查找成功即保证指针指向有效的 `GraphicsOutput`。
    let mode = unsafe { (*output).mode };
    if mode.is_null() {
        return None;
    }
    // SAFETY: 固件保证 `mode` 在引导服务期间有效。
    Some(unsafe { &*mode })
}

#[cfg(test)]
mod graphics_output_tests {
    use super::{GraphicsOutput, ModeInformation, PixelBitmask, graphics_output_mode};
    use crate::discover::SEARCH_TYPE_BY_PROTOCOL;
    use crate::types::{BUFFER_TOO_SMALL, Handle, Status, SUCCESS};
        use core::mem::size_of;

    static INFO: ModeInformation = ModeInformation {
        version: 0,
        horizontal_resolution: 1024,
        vertical_resolution: 768,
        pixel_format: 0,
        pixel_information: PixelBitmask {
            red_mask: 0x00FF_0000,
            green_mask: 0x0000_FF00,
            blue_mask: 0x0000_00FF,
            reserved_mask: 0,
        },
        pixels_per_scan_line: 1024,
    };

    static mut MODE: super::GraphicsOutputMode = super::GraphicsOutputMode {
        max_mode: 1,
        mode: 0,
        info: core::ptr::null_mut(),
        size_of_info: 0,
        framebuffer_base: 0xFD00_0000,
        framebuffer_size: 0x30_0000,
    };

    static mut OUTPUT: GraphicsOutput = GraphicsOutput {
        query_mode: core::ptr::null_mut(),
        set_mode: core::ptr::null_mut(),
        blt: core::ptr::null_mut(),
        mode: core::ptr::null_mut(),
    };

    unsafe extern "efiapi" fn fake_locate(
        search_type: u32,
        _protocol: *const c_void,
        _key: *mut c_void,
        size: *mut usize,
        buffer: *mut Handle,
    ) -> Status {
        assert_eq!(search_type, SEARCH_TYPE_BY_PROTOCOL);
        let unit = size_of::<Handle>();
        // SAFETY: 调用方按 UEFI 契约传入有效指针。
        unsafe {
            if buffer.is_null() {
                *size = unit;
                BUFFER_TOO_SMALL
            } else {
                *size = unit;
                *buffer = 0x20usize as *mut c_void;
                SUCCESS
            }
        }
    }

    unsafe extern "efiapi" fn fake_handle_protocol(
        _handle: Handle,
        _protocol: *const c_void,
        interface: *mut *mut c_void,
    ) -> Status {
        // SAFETY: 同上；把假的 GOP 交出去。
        unsafe { *interface = core::ptr::addr_of_mut!(OUTPUT) as *mut c_void };
        SUCCESS
    }

    #[test]
    fn the_graphics_output_mode_is_found_through_the_firmware() {
        // SAFETY: 测试内单线程初始化静态夹具。
        unsafe {
            MODE.info = core::ptr::addr_of!(INFO) as *mut ModeInformation;
            OUTPUT.mode = core::ptr::addr_of_mut!(MODE);
        }
        // SAFETY: 引导阶段单线程调用一次；此处即测试线程。
        let mode = unsafe { graphics_output_mode(fake_locate, fake_handle_protocol) }
            .expect("必须找到 GOP 模式");
        assert_eq!(mode.framebuffer_base, 0xFD00_0000);
        assert!(!mode.info.is_null());
        // SAFETY: `info` 指向静态夹具。
        assert_eq!(unsafe { (*mode.info).horizontal_resolution }, 1024);
    }
use core::ffi::c_void;
}