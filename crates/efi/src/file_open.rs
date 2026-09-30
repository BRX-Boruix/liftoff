//! 文件打开编排：路径校验 → UTF-16 转换 → 调用固件 → 句柄校验。
//!
//! 边界：只做**打开**；读取由 `FileSource` 实现调用 `FileProtocol::read` 完成。

use crate::file::FileProtocol;
use crate::status::status_to_error;
use crate::types::Status;
use firmware::error::Error;
use firmware::file::validate_path;

/// `EFI_FILE_OPEN` 的签名（只用到只读打开）。
pub type Open = unsafe extern "efiapi" fn(
    this: *mut FileProtocol,
    new_handle: *mut *mut FileProtocol,
    file_name: *mut u16,
    open_mode: u64,
    attributes: u64,
) -> Status;

/// 只读打开模式（UEFI 规范：`EFI_FILE_MODE_READ`）。
pub const OPEN_MODE_READ: u64 = 0x0000_0000_0000_0001;

/// 把抽象层路径转成 UEFI 的宽字符串（UTF-16 + 结尾 NUL），并把 `/` 映射为 `\`。
///
/// 返回**不含 NUL 的长度**；缓冲放不下（含 NUL）时返回 `Error::BufferTooSmall`。
pub fn to_wide(path: &str, wide: &mut [u16]) -> Result<usize, Error> {
    let mut len = 0;
    for unit in path.encode_utf16() {
        if len + 1 >= wide.len() {
            return Err(Error::BufferTooSmall);
        }
        wide[len] = if unit == b'/' as u16 {
            b'\\' as u16
        } else {
            unit
        };
        len += 1;
    }
    if len >= wide.len() {
        return Err(Error::BufferTooSmall);
    }
    wide[len] = 0;
    Ok(len)
}

/// 在卷根上按路径打开文件。
pub fn open_on_volume(
    root: *mut FileProtocol,
    open: Open,
    path: &str,
    wide: &mut [u16],
) -> Result<*mut FileProtocol, Error> {
    validate_path(path)?;
    to_wide(path, wide)?;
    let mut handle: *mut FileProtocol = core::ptr::null_mut();
    // SAFETY: `root` 由调用方保证是有效的卷根 `FileProtocol`；`wide` 以 NUL 结尾；
    // `handle` 指向本栈上的有效变量。
    let status = unsafe { open(root, &mut handle, wide.as_mut_ptr(), OPEN_MODE_READ, 0) };
    if let Some(err) = status_to_error(status) {
        return Err(err);
    }
    if handle.is_null() {
        return Err(Error::Io);
    }
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::{OPEN_MODE_READ, open_on_volume, to_wide};
    use crate::file::FileProtocol;
    use crate::types::{Status, SUCCESS};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_open(
        _this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        _name: *mut u16,
        mode: u64,
        _attrs: u64,
    ) -> Status {
        assert_eq!(mode, OPEN_MODE_READ, "打开模式必须是只读");
        CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: 调用方按契约传入有效指针。
        unsafe { *new_handle = 0x1000usize as *mut FileProtocol };
        SUCCESS
    }

    #[test]
    fn to_wide_converts_and_maps_separators() {
        let mut wide = [0u16; 32];
        let len = to_wide("EFI/BOOT/BOOTX64.EFI", &mut wide).expect("转换成功");
        assert_eq!(len, 20);
        assert_eq!(wide[0], 'E' as u16);
        assert_eq!(wide[3], '\\' as u16, "抽象层用 /，UEFI 用 \\，必须映射");
        assert_eq!(wide[len], 0, "必须带结尾 NUL");
    }

    #[test]
    fn to_wide_reports_a_too_small_buffer() {
        let mut wide = [0u16; 4];
        assert_eq!(to_wide("kernel", &mut wide), Err(Error::BufferTooSmall));
    }

    #[test]
    fn open_validates_the_path_before_calling_firmware() {
        CALLS.store(0, Ordering::SeqCst);
        let mut wide = [0u16; 32];
        assert_eq!(
            open_on_volume(core::ptr::null_mut(), fake_open, "/abs", &mut wide),
            Err(Error::InvalidArgument)
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "路径非法时不得调用固件");
        let handle = open_on_volume(core::ptr::null_mut(), fake_open, "kernel", &mut wide).expect("打开成功");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert!(!handle.is_null());
    }
}
