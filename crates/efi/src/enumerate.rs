//! 两段式句柄枚举编排。
//!
//! 编排：① 以 `buffer_size = 0` 探测所需字节数（固件回 `BUFFER_TOO_SMALL`）；
//! ② 用 `handle_count` 换算句柄数并校验调用方缓冲；③ 枚举；④ 复核固件报告的实际字节数。
//!
//! `LocateHandle` 以参数注入，因此整条编排可在宿主上用假固件测透。

use crate::boot_services_table::LocateHandle;
use crate::handles::handle_count;
use crate::status::status_to_error;
use crate::types::{BUFFER_TOO_SMALL, Handle, SUCCESS};
use core::ffi::c_void;
use firmware::error::Error;

/// 枚举匹配的句柄，返回句柄数。
pub fn enumerate_handles(
    locate_handle: LocateHandle,
    search_type: u32,
    protocol: *const c_void,
    buffer: &mut [Handle],
) -> Result<usize, Error> {
    let mut needed: usize = 0;
    // SAFETY: 探测用法 —— buffer 为 null 且 size 为 0；其余指针指向本栈上的有效变量。
    let probe = unsafe {
        locate_handle(
            search_type,
            protocol,
            core::ptr::null_mut(),
            &mut needed,
            core::ptr::null_mut(),
        )
    };
    if probe != SUCCESS && probe != BUFFER_TOO_SMALL {
        return Err(status_to_error(probe).unwrap_or(Error::Io));
    }
    let count = handle_count(needed).ok_or(Error::Io)?;
    if count > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    let mut size = needed;
    // SAFETY: `buffer` 是可写缓冲且 `size` 为其字节长度；其余指针指向本栈上的有效变量。
    let taken = unsafe {
        locate_handle(
            search_type,
            protocol,
            core::ptr::null_mut(),
            &mut size,
            buffer.as_mut_ptr(),
        )
    };
    if let Some(err) = status_to_error(taken) {
        return Err(err);
    }
    let actual = handle_count(size).ok_or(Error::Io)?;
    if actual > buffer.len() {
        return Err(Error::Io);
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::enumerate_handles;
    use crate::types::{BUFFER_TOO_SMALL, Handle, Status, SUCCESS};
    use core::ffi::c_void;
    use core::mem::size_of;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static MISALIGN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_locate(
        _search_type: u32,
        _protocol: *const c_void,
        _key: *mut c_void,
        size: *mut usize,
        buffer: *mut Handle,
    ) -> Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        let unit = size_of::<Handle>();
        // SAFETY: 调用方按 UEFI 契约传入有效指针；测试中始终如此。
        unsafe {
            if MISALIGN.load(Ordering::SeqCst) == 1 {
                *size = unit * 2 + 1;
                return SUCCESS;
            }
            if buffer.is_null() {
                *size = unit * 2;
                BUFFER_TOO_SMALL
            } else {
                *size = unit * 2;
                *buffer = core::ptr::null_mut();
                *buffer.add(1) = 1usize as Handle;
                SUCCESS
            }
        }
    }

    #[test]
    fn two_stage_enumeration_returns_the_handle_count() {
        CALLS.store(0, Ordering::SeqCst);
        MISALIGN.store(0, Ordering::SeqCst);
        let mut buffer = [core::ptr::null_mut(); 4];
        let count = enumerate_handles(fake_locate, 0, core::ptr::null(), &mut buffer).expect("枚举成功");
        assert_eq!(count, 2);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "应当探测一次、枚举一次");
    }

    #[test]
    fn too_small_buffer_stops_after_the_probe() {
        CALLS.store(0, Ordering::SeqCst);
        MISALIGN.store(0, Ordering::SeqCst);
        let mut buffer = [core::ptr::null_mut(); 1];
        assert_eq!(enumerate_handles(fake_locate, 0, core::ptr::null(), &mut buffer), Err(Error::BufferTooSmall));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "容量不足时不应发起第二次调用");
    }

    #[test]
    fn misaligned_size_is_treated_as_a_firmware_violation() {
        CALLS.store(0, Ordering::SeqCst);
        MISALIGN.store(1, Ordering::SeqCst);
        let mut buffer = [core::ptr::null_mut(); 4];
        assert_eq!(enumerate_handles(fake_locate, 0, core::ptr::null(), &mut buffer), Err(Error::Io));
    }
}
