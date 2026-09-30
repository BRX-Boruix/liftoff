//! 引导服务：我们使用的调用签名与相关算术。
//!
//! 边界：只放**真正调用的签名**与**纯算术**；完整的 `EFI_BOOT_SERVICES` 表在需要调用时
//! 再按规范逐字段补齐（不预置未使用的函数指针）。

use crate::types::Status;
use core::ffi::c_void;

/// `GetMemoryMap` 的签名（UEFI 规范）。
///
/// 语义：先以 `map_size` 为 0 调用，固件返回 `EFI_BUFFER_TOO_SMALL` 并写入所需字节数；
/// 再按该大小分配缓冲重试。`map_key` 必须在退出引导服务时原样传回。
pub type GetMemoryMap = unsafe extern "efiapi" fn(
    map_size: *mut usize,
    map: *mut c_void,
    map_key: *mut usize,
    descriptor_size: *mut usize,
    descriptor_version: *mut u32,
) -> Status;

/// 由固件返回的缓冲字节数与描述符大小，算出可容纳的**完整**描述符数（向下取整）。
///
/// `map_size == 0` 或 `descriptor_size == 0` 返回 `None`：前者说明固件未给出有效大小，
/// 后者会导致除零 —— 两者都是状态错误，不做静默兜底（宁可报错）。
pub const fn descriptor_capacity(map_size: usize, descriptor_size: usize) -> Option<usize> {
    if map_size == 0 || descriptor_size == 0 {
        return None;
    }
    Some(map_size / descriptor_size)
}

#[cfg(test)]
mod tests {
    use super::descriptor_capacity;

    #[test]
    fn capacity_is_the_floor_of_size_over_descriptor() {
        assert_eq!(descriptor_capacity(48, 48), Some(1));
        assert_eq!(descriptor_capacity(96, 48), Some(2));
        assert_eq!(descriptor_capacity(4096, 48), Some(85));
    }

    #[test]
    fn a_partial_descriptor_does_not_count() {
        assert_eq!(descriptor_capacity(47, 48), Some(0));
        assert_eq!(descriptor_capacity(95, 48), Some(1));
    }

    #[test]
    fn zero_inputs_are_rejected() {
        assert_eq!(descriptor_capacity(0, 48), None);
        assert_eq!(descriptor_capacity(48, 0), None);
        assert_eq!(descriptor_capacity(0, 0), None);
    }
}
