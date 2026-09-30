//! 打开文件并取信息的编排。
//!
//! **失败路径必须关闭已打开的句柄**（S18 资源生命周期显式化）：取信息失败时先关闭，
//! 再上报原始错误；关闭本身的失败不改变错误语义。

use crate::file::{Close, FileProtocol};
use crate::file_get_info::file_info_of;
use crate::file_info::GetInfo;
use crate::file_io::close_once;
use crate::file_open::{Open, open_on_volume};
use firmware::error::Error;
use firmware::file::FileInfo;

/// 打开文件并取信息；取信息失败时关闭已打开的句柄后再上报原始错误。
pub fn open_with_info(
    root: *mut FileProtocol,
    open: Open,
    close: Close,
    get_info: GetInfo,
    path: &str,
    wide: &mut [u16],
    info_buffer: &mut [u8],
) -> Result<(*mut FileProtocol, FileInfo), Error> {
    let file = open_on_volume(root, open, path, wide)?;
    match file_info_of(get_info, file, info_buffer) {
        Ok(info) => Ok((file, info)),
        Err(err) => {
            // 关闭失败不改变错误语义：上报原始错误（至少不泄漏句柄）。
            let _ = close_once(close, file);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::open_with_info;
    use crate::file::{Close, FileProtocol};
    use crate::file_info::{FileInfo as EfiFileInfo, GetInfo, Time};
    use crate::file_open::Open;
    use crate::guid::Guid;
    use crate::types::{DEVICE_ERROR, Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static CLOSED: AtomicUsize = AtomicUsize::new(0);
    static FAIL_INFO: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_open(
        _this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        _name: *mut u16,
        _mode: u64,
        _attrs: u64,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针。
        unsafe { *new_handle = 0x3000usize as *mut FileProtocol };
        SUCCESS
    }

    unsafe extern "efiapi" fn fake_close(_this: *mut FileProtocol) -> Status {
        CLOSED.fetch_add(1, Ordering::SeqCst);
        SUCCESS
    }

    unsafe extern "efiapi" fn fake_get_info(
        _this: *mut FileProtocol,
        _t: *const Guid,
        size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        if FAIL_INFO.load(Ordering::SeqCst) == 1 {
            return DEVICE_ERROR;
        }
        // SAFETY: 调用方按契约传入有效指针。
        unsafe {
            if buffer.is_null() {
                *size = 80;
                crate::types::BUFFER_TOO_SMALL
            } else {
                let t = Time {
                    year: 2026,
                    month: 9,
                    day: 30,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    pad1: 0,
                    nanosecond: 0,
                    timezone: 0,
                    daylight: 0,
                    pad2: 0,
                };
                *size = 80;
                core::ptr::write_unaligned(
                    buffer.cast::<EfiFileInfo>(),
                    EfiFileInfo {
                        size: 80,
                        file_size: 2048,
                        physical_size: 2048,
                        create_time: t,
                        last_access_time: t,
                        modification_time: t,
                        attribute: 0,
                    },
                );
                SUCCESS
            }
        }
    }

    fn open_fn() -> Open {
        fake_open
    }

    fn close_fn() -> Close {
        fake_close
    }

    fn info_fn() -> GetInfo {
        fake_get_info
    }

    #[test]
    fn a_successful_open_returns_the_file_and_its_info() {
        CLOSED.store(0, Ordering::SeqCst);
        FAIL_INFO.store(0, Ordering::SeqCst);
        let mut wide = [0u16; 32];
        let mut info_buffer = [0u8; 128];
        let (file, info) = open_with_info(
            core::ptr::null_mut(),
            open_fn(),
            close_fn(),
            info_fn(),
            "kernel",
            &mut wide,
            &mut info_buffer,
        )
        .expect("打开成功");
        assert!(!file.is_null());
        assert_eq!(info.size, 2048);
        assert_eq!(CLOSED.load(Ordering::SeqCst), 0, "成功路径不应关闭");
    }

    #[test]
    fn a_failing_info_query_closes_the_already_opened_file() {
        CLOSED.store(0, Ordering::SeqCst);
        FAIL_INFO.store(1, Ordering::SeqCst);
        let mut wide = [0u16; 32];
        let mut info_buffer = [0u8; 128];
        assert_eq!(
            open_with_info(
                core::ptr::null_mut(),
                open_fn(),
                close_fn(),
                info_fn(),
                "kernel",
                &mut wide,
                &mut info_buffer,
            ),
            Err(Error::Io)
        );
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1, "失败路径必须关闭已打开的句柄");
    }
}
