//! 文件读取与关闭编排。
//!
//! 边界：只做**一次读**与**一次关闭**；打开由 `file_open` 负责，卷根由调用方提供。

use crate::file::{Close, FileProtocol, Read};
use crate::status::status_to_error;
use core::ffi::c_void;
use firmware::error::Error;
use crate::file::validate_read_result;

/// 读一次文件：调用固件读入缓冲，并按**短读契约**校验返回的字节数。
pub fn read_once(read: Read, file: *mut FileProtocol, buffer: &mut [u8]) -> Result<usize, Error> {
    let mut size = buffer.len();
    // SAFETY: `file` 由调用方保证是有效句柄；`size` 与 `buffer` 长度一致，且固件按契约
    // 只写不超过该长度的字节（返回值另行校验）。
    let status = unsafe { read(file, &mut size, buffer.as_mut_ptr().cast::<c_void>()) };
    if let Some(err) = status_to_error(status) {
        return Err(err);
    }
    validate_read_result(buffer.len(), size)
}

/// 关闭句柄。
pub fn close_once(close: Close, file: *mut FileProtocol) -> Result<(), Error> {
    // SAFETY: `file` 由调用方保证是有效句柄。
    let status = unsafe { close(file) };
    match status_to_error(status) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{close_once, read_once};
    use crate::file::{Close, FileProtocol, Read};
    use crate::types::{DEVICE_ERROR, Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static MODE: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_read(
        _this: *mut FileProtocol,
        size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针。
        unsafe {
            match MODE.load(Ordering::SeqCst) {
                0 => {
                    *size = 3;
                    core::ptr::write_bytes(buffer.cast::<u8>(), 0x77, 3);
                    SUCCESS
                }
                1 => {
                    *size = 99;
                    SUCCESS
                }
                _ => DEVICE_ERROR,
            }
        }
    }

    unsafe extern "efiapi" fn fake_close(_this: *mut FileProtocol) -> Status {
        if MODE.load(Ordering::SeqCst) == 2 {
            DEVICE_ERROR
        } else {
            SUCCESS
        }
    }

    fn read_fn() -> Read {
        fake_read
    }

    fn close_fn() -> Close {
        fake_close
    }

    #[test]
    fn read_returns_the_actual_byte_count() {
        MODE.store(0, Ordering::SeqCst);
        let mut buffer = [0u8; 8];
        let got = read_once(read_fn(), core::ptr::null_mut(), &mut buffer).expect("读成功");
        assert_eq!(got, 3, "短读是允许的");
        assert_eq!(buffer[0], 0x77);
    }

    #[test]
    fn read_rejecting_a_size_beyond_the_buffer_is_a_violation() {
        MODE.store(1, Ordering::SeqCst);
        let mut buffer = [0u8; 8];
        assert_eq!(read_once(read_fn(), core::ptr::null_mut(), &mut buffer), Err(Error::Io));
    }

    #[test]
    fn close_maps_the_status() {
        MODE.store(0, Ordering::SeqCst);
        assert_eq!(close_once(close_fn(), core::ptr::null_mut()), Ok(()));
        MODE.store(2, Ordering::SeqCst);
        assert_eq!(close_once(close_fn(), core::ptr::null_mut()), Err(Error::Io));
    }
}
