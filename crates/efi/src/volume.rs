//! 卷根打开编排。
//!
//! 边界：只做**打开卷根**；文件打开由 `file_open` 负责。

use crate::file::{FileProtocol, OpenVolume, SimpleFileSystem};
use crate::status::status_to_error;
use firmware::error::Error;

/// 打开卷根：调用注入的 `OpenVolume` 并校验返回的根句柄非空。
pub fn open_volume_root(
    open_volume: OpenVolume,
    this: *mut SimpleFileSystem,
) -> Result<*mut FileProtocol, Error> {
    let mut root: *mut FileProtocol = core::ptr::null_mut();
    // SAFETY: `this` 由调用方保证是有效的 `SimpleFileSystem`；`root` 指向本栈上的有效变量。
    let status = unsafe { open_volume(this, &mut root) };
    if let Some(err) = status_to_error(status) {
        return Err(err);
    }
    if root.is_null() {
        return Err(Error::Io);
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::open_volume_root;
    use crate::file::{FileProtocol, SimpleFileSystem};
    use crate::types::{Status, SUCCESS, UNSUPPORTED};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static MODE: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_open_volume(
        _this: *mut SimpleFileSystem,
        root: *mut *mut FileProtocol,
    ) -> Status {
        match MODE.load(Ordering::SeqCst) {
            0 => {
                // SAFETY: 调用方按契约传入有效指针。
                unsafe { *root = 0x2000usize as *mut FileProtocol };
                SUCCESS
            }
            1 => {
                // SAFETY: 调用方按契约传入有效指针。
                unsafe { *root = core::ptr::null_mut() };
                SUCCESS
            }
            _ => UNSUPPORTED,
        }
    }

    #[test]
    fn a_valid_volume_root_is_returned() {
        MODE.store(0, Ordering::SeqCst);
        let root = open_volume_root(fake_open_volume, core::ptr::null_mut()).expect("卷根可打开");
        assert!(!root.is_null());
    }

    #[test]
    fn null_root_and_firmware_error_are_rejected() {
        MODE.store(1, Ordering::SeqCst);
        assert_eq!(open_volume_root(fake_open_volume, core::ptr::null_mut()), Err(Error::Io));
        MODE.store(2, Ordering::SeqCst);
        assert_eq!(
            open_volume_root(fake_open_volume, core::ptr::null_mut()),
            Err(Error::Unsupported)
        );
    }
}
