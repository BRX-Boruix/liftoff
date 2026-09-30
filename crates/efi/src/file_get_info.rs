//! `GetInfo` 编排：取 `EFI_FILE_INFO` 并提取抽象文件信息。
//!
//! 两段式：先以 0 大小探测所需缓冲（固件回 `BUFFER_TOO_SMALL`），再按该大小取数据。
//! 缓冲由调用方持有，**无隐式分配**。

use crate::file::FileProtocol;
use crate::file_info::{FILE_ATTRIBUTE_READ_ONLY, FILE_INFO_ID, FileInfo as EfiFileInfo, GetInfo};
use crate::status::status_to_error;
use crate::types::{BUFFER_TOO_SMALL, SUCCESS};
use core::ffi::c_void;
use core::mem::size_of;
use firmware::error::Error;
use firmware::file::FileInfo;

/// 取文件信息（大小与只读标志）。
pub fn file_info_of(
    get_info: GetInfo,
    file: *mut FileProtocol,
    buffer: &mut [u8],
) -> Result<FileInfo, Error> {
    let mut needed: usize = 0;
    // SAFETY: 探测用法 —— buffer 为 null 且 size 为 0；其余指针指向本栈上的有效变量。
    let probe = unsafe { get_info(file, &FILE_INFO_ID, &mut needed, core::ptr::null_mut()) };
    if probe != SUCCESS && probe != BUFFER_TOO_SMALL {
        return Err(status_to_error(probe).unwrap_or(Error::Io));
    }
    if needed == 0 || needed > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    let mut size = buffer.len();
    // SAFETY: `buffer` 是可写缓冲且 `size` 为其长度；其余指针指向本栈上的有效变量。
    let status = unsafe {
        get_info(file, &FILE_INFO_ID, &mut size, buffer.as_mut_ptr().cast::<c_void>())
    };
    if let Some(err) = status_to_error(status) {
        return Err(err);
    }
    // 固件报告的大小必须装得下结构前缀，否则视为违约（不读越界字段）。
    if size < size_of::<EfiFileInfo>() {
        return Err(Error::Io);
    }
    // SAFETY: `size >= size_of::<EfiFileInfo>()`，缓冲至少有 `needed` 字节；用 `read_unaligned`
    // 因为缓冲不保证按 8 字节对齐。
    let info = unsafe { core::ptr::read_unaligned(buffer.as_ptr().cast::<EfiFileInfo>()) };
    Ok(FileInfo {
        size: info.file_size,
        read_only: info.attribute & FILE_ATTRIBUTE_READ_ONLY != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::file_info_of;
    use crate::file::FileProtocol;
    use crate::file_info::{FILE_ATTRIBUTE_READ_ONLY, FileInfo as EfiFileInfo, Time};
    use crate::guid::Guid;
    use crate::types::{BUFFER_TOO_SMALL, DEVICE_ERROR, Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static MODE: AtomicUsize = AtomicUsize::new(0);

    fn sample(attribute: u64) -> EfiFileInfo {
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
        EfiFileInfo {
            size: 80,
            file_size: 4096,
            physical_size: 4096,
            create_time: t,
            last_access_time: t,
            modification_time: t,
            attribute,
        }
    }

    unsafe extern "efiapi" fn fake_get_info(
        _this: *mut FileProtocol,
        info_type: *const Guid,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针；此处校验 GUID 指针非空。
        unsafe {
            assert!(!info_type.is_null(), "必须传 EFI_FILE_INFO 的 GUID");
            match MODE.load(Ordering::SeqCst) {
                0 => {
                    if buffer.is_null() {
                        *buffer_size = 80;
                        BUFFER_TOO_SMALL
                    } else {
                        *buffer_size = 80;
                        core::ptr::write_unaligned(
                            buffer.cast::<EfiFileInfo>(),
                            sample(FILE_ATTRIBUTE_READ_ONLY),
                        );
                        SUCCESS
                    }
                }
                1 => {
                    *buffer_size = 80;
                    BUFFER_TOO_SMALL
                }
                _ => DEVICE_ERROR,
            }
        }
    }

    #[test]
    fn file_size_and_read_only_are_extracted() {
        MODE.store(0, Ordering::SeqCst);
        let mut buffer = [0u8; 128];
        let info = file_info_of(fake_get_info, core::ptr::null_mut(), &mut buffer).expect("取信息");
        assert_eq!(info.size, 4096);
        assert!(info.read_only, "属性位含只读位");
    }

    #[test]
    fn a_too_small_buffer_stops_after_the_probe() {
        MODE.store(1, Ordering::SeqCst);
        let mut buffer = [0u8; 16];
        assert_eq!(
            file_info_of(fake_get_info, core::ptr::null_mut(), &mut buffer),
            Err(Error::BufferTooSmall)
        );
    }

    #[test]
    fn firmware_errors_are_mapped() {
        MODE.store(2, Ordering::SeqCst);
        let mut buffer = [0u8; 128];
        assert_eq!(file_info_of(fake_get_info, core::ptr::null_mut(), &mut buffer), Err(Error::Io));
    }
}
