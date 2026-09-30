//! 句柄枚举的算术：`LocateHandle` 返回的字节数 → 句柄数。
//!
//! 两段式用法：先以 `buffer_size = 0` 调用探测所需字节数（固件回 `BUFFER_TOO_SMALL`），
//! 再按该大小提供缓冲枚举。这里只做**换算与违约检查**（纯算术，宿主可测）。

use crate::types::Handle;

/// 由 `LocateHandle` 报告的字节数换算句柄数。
///
/// - `0` → `Some(0)`：**没有设备是合法状态**，不是错误；
/// - 不是 `size_of::<Handle>()` 的整数倍 → `None`：固件给出的字节数与句柄布局不符（违约），
///   不能按截断结果继续。
pub const fn handle_count(bytes: usize) -> Option<usize> {
    let unit = core::mem::size_of::<Handle>();
    if bytes % unit != 0 {
        return None;
    }
    Some(bytes / unit)
}

#[cfg(test)]
mod tests {
    use super::handle_count;
    use crate::types::Handle;
    use core::mem::size_of;

    #[test]
    fn byte_size_converts_to_handle_count() {
        let unit = size_of::<Handle>();
        assert_eq!(handle_count(unit), Some(1));
        assert_eq!(handle_count(unit * 3), Some(3));
    }

    #[test]
    fn no_devices_is_a_legitimate_empty_result() {
        assert_eq!(handle_count(0), Some(0));
    }

    #[test]
    fn misaligned_byte_sizes_are_rejected() {
        assert_eq!(handle_count(1), None, "不是句柄大小的整数倍即固件违约");
        assert_eq!(handle_count(size_of::<Handle>() + 1), None);
    }
}
