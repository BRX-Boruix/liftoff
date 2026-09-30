//! 块设备发现：枚举句柄并逐个解析 `BlockIo`。
//!
//! 边界：只做“发现 + 解析”；`BlockDeviceSource` 的实现用它填充自己的句柄表。
//!
//! 协议查找失败**不静默跳过**：既然按 `BlockIo` 协议查找句柄，每个句柄都应暴露它，
//! 失败即固件不一致 → 报错。

use crate::block_io::BlockIo;
use crate::boot_services_table::{HandleProtocol, LocateHandle};
use crate::enumerate::enumerate_handles;
use crate::guid::{BLOCK_IO_PROTOCOL, Guid};
use crate::protocol_lookup::block_io_on_handle;
use crate::types::Handle;
use core::ffi::c_void;
use firmware::error::Error;

/// `LocateHandle` 的 `ByProtocol` 搜索类型（UEFI 规范）。
pub const SEARCH_TYPE_BY_PROTOCOL: u32 = 2;

/// 枚举所有暴露 `BlockIo` 的句柄，把解析出的 `*mut BlockIo` 填入 `devices`，返回设备数。
pub fn discover_block_devices(
    locate_handle: LocateHandle,
    handle_protocol: HandleProtocol,
    handles: &mut [Handle],
    devices: &mut [*mut BlockIo],
) -> Result<usize, Error> {
    let guid = (&BLOCK_IO_PROTOCOL as *const Guid).cast::<c_void>();
    let count = enumerate_handles(locate_handle, SEARCH_TYPE_BY_PROTOCOL, guid, handles)?;
    if count > devices.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, handle) in handles.iter().take(count).enumerate() {
        devices[index] = block_io_on_handle(handle_protocol, *handle, guid)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{SEARCH_TYPE_BY_PROTOCOL, discover_block_devices};
    use crate::block_io::{BlockIo, BlockIoMedia};
    use crate::types::{Handle, Status, SUCCESS, UNSUPPORTED};
    use core::ffi::c_void;
    use core::mem::size_of;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;

    static FAIL: AtomicUsize = AtomicUsize::new(0);

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

    unsafe extern "efiapi" fn fake_locate(
        search_type: u32,
        _protocol: *const c_void,
        _key: *mut c_void,
        size: *mut usize,
        buffer: *mut Handle,
    ) -> Status {
        assert_eq!(search_type, SEARCH_TYPE_BY_PROTOCOL);
        let unit = size_of::<Handle>();
        // SAFETY: 调用方按 UEFI 契约传入有效指针；测试中始终如此。
        unsafe {
            if buffer.is_null() {
                *size = unit;
                crate::types::BUFFER_TOO_SMALL
            } else {
                *size = unit;
                *buffer = 0x10usize as Handle;
                SUCCESS
            }
        }
    }

    unsafe extern "efiapi" fn fake_handle_protocol(
        _handle: Handle,
        _protocol: *const c_void,
        interface: *mut *mut c_void,
    ) -> Status {
        if FAIL.load(Ordering::SeqCst) == 1 {
            return UNSUPPORTED;
        }
        // SAFETY: 测试内单线程；写入有效指针，不创建静态引用。
        unsafe {
            (*core::ptr::addr_of_mut!(IO)).media = (&MEDIA as *const BlockIoMedia).cast_mut();
            *interface = core::ptr::addr_of_mut!(IO).cast::<c_void>();
        }
        SUCCESS
    }

    #[test]
    fn a_block_device_is_discovered() {
        FAIL.store(0, Ordering::SeqCst);
        let mut handles = [core::ptr::null_mut(); 4];
        let mut devices = [core::ptr::null_mut(); 4];
        let found = discover_block_devices(fake_locate, fake_handle_protocol, &mut handles, &mut devices)
            .expect("应发现一个设备");
        assert_eq!(found, 1);
        assert!(!devices[0].is_null());
    }

    #[test]
    fn a_failing_protocol_lookup_is_reported_not_skipped() {
        FAIL.store(1, Ordering::SeqCst);
        let mut handles = [core::ptr::null_mut(); 4];
        let mut devices = [core::ptr::null_mut(); 4];
        assert_eq!(
            discover_block_devices(fake_locate, fake_handle_protocol, &mut handles, &mut devices),
            Err(Error::Unsupported)
        );
    }
}
