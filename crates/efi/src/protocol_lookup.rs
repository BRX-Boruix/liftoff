//! `HandleProtocol` 的查询编排。
//!
//! 编排：调用注入的 `HandleProtocol` → 映射返回码 → 校验接口指针（复用 `protocol` 模块）。
//!
//! `HandleProtocol` 以参数注入，因此可在宿主上用假固件测透。

use crate::block_io::BlockIo;
use crate::boot_services_table::HandleProtocol;
use crate::protocol::block_io_from_interface;
use crate::status::status_to_error;
use crate::types::Handle;
use core::ffi::c_void;
use firmware::error::Error;

/// 查询句柄上的 `BlockIo` 协议。
pub fn block_io_on_handle(
    handle_protocol: HandleProtocol,
    handle: Handle,
    protocol: *const c_void,
) -> Result<*mut BlockIo, Error> {
    let mut interface: *mut c_void = core::ptr::null_mut();
    // SAFETY: `handle` 由调用方保证来自枚举结果；`protocol` 指向有效的 GUID 常量；
    // `interface` 指向本栈上的有效变量。
    let status = unsafe { handle_protocol(handle, protocol, &mut interface) };
    if let Some(err) = status_to_error(status) {
        return Err(err);
    }
    block_io_from_interface(interface)
}

#[cfg(test)]
mod tests {
    use super::block_io_on_handle;
    use crate::block_io::{BlockIo, BlockIoMedia};
    use crate::types::{Handle, Status, SUCCESS, UNSUPPORTED};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static MODE: AtomicUsize = AtomicUsize::new(0);

    static MEDIA: BlockIoMedia = BlockIoMedia {
        media_id: 1,
        removable_media: 0,
        media_present: 1,
        logical_partition: 0,
        read_only: 0,
        write_caching: 0,
        block_size: 512,
        io_align: 0,
        last_block: 7,
        lowest_aligned_lba: 0,
        logical_blocks_per_physical_block: 1,
        optimal_transfer_length_granularity: 0,
    };

    unsafe extern "efiapi" fn unused_read(
        _this: *mut BlockIo,
        _media_id: u32,
        _lba: u64,
        _size: usize,
        _buffer: *mut c_void,
    ) -> Status {
        SUCCESS
    }

    static mut IO: BlockIo = BlockIo {
        revision: 1,
        media: core::ptr::null_mut(),
        reset: core::ptr::null_mut(),
        read_blocks: unused_read,
        write_blocks: core::ptr::null_mut(),
        flush_blocks: core::ptr::null_mut(),
    };

    unsafe extern "efiapi" fn fake_handle_protocol(
        _handle: Handle,
        _protocol: *const c_void,
        interface: *mut *mut c_void,
    ) -> Status {
        match MODE.load(Ordering::SeqCst) {
            0 => {
                // SAFETY: 测试内单线程使用该静态；此处写入有效指针，不创建引用。
                unsafe {
                    (*core::ptr::addr_of_mut!(IO)).media =
                        (&MEDIA as *const BlockIoMedia).cast_mut();
                    *interface = core::ptr::addr_of_mut!(IO).cast::<c_void>();
                }
                SUCCESS
            }
            1 => {
                // SAFETY: 调用方保证 interface 有效。
                unsafe { *interface = core::ptr::null_mut() };
                SUCCESS
            }
            _ => UNSUPPORTED,
        }
    }

    #[test]
    fn a_handle_exposing_block_io_is_resolved() {
        MODE.store(0, Ordering::SeqCst);
        let got = block_io_on_handle(fake_handle_protocol, core::ptr::null_mut(), core::ptr::null())
            .expect("应解析出 BlockIo");
        assert!(!got.is_null());
    }

    #[test]
    fn firmware_error_is_mapped() {
        MODE.store(2, Ordering::SeqCst);
        assert_eq!(
            block_io_on_handle(fake_handle_protocol, core::ptr::null_mut(), core::ptr::null()),
            Err(Error::Unsupported)
        );
    }

    #[test]
    fn success_with_null_interface_is_a_violation() {
        MODE.store(1, Ordering::SeqCst);
        assert_eq!(
            block_io_on_handle(fake_handle_protocol, core::ptr::null_mut(), core::ptr::null()),
            Err(Error::Io)
        );
    }
}
