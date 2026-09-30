//! 协议接口指针的校验。
//!
//! 边界：只做**指针校验**（非空、`Media` 非空）；协议查找本身由 `BlockDeviceSource`
//! 的实现调用 `HandleProtocol` 完成。

use crate::block_io::BlockIo;
use core::ffi::c_void;
use firmware::error::Error;

/// 校验 `HandleProtocol` 返回的接口指针并取出 `BlockIo`。
///
/// 固件返回成功但接口为空、或 `Media` 为空，都是**固件违约** → `Error::Io`；
/// 绝不 deref 空指针。
pub fn block_io_from_interface(interface: *mut c_void) -> Result<*mut BlockIo, Error> {
    if interface.is_null() {
        return Err(Error::Io);
    }
    let block_io = interface.cast::<BlockIo>();
    // SAFETY: 指针非空，且由固件保证在协议查找成功时指向有效的 `BlockIo`。
    let media = unsafe { (*block_io).media };
    if media.is_null() {
        return Err(Error::Io);
    }
    Ok(block_io)
}

#[cfg(test)]
mod tests {
    use super::block_io_from_interface;
    use crate::block_io::{BlockIo, BlockIoMedia};
    use core::ffi::c_void;
    use firmware::error::Error;

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
    ) -> crate::types::Status {
        crate::types::SUCCESS
    }

    fn block_io(media: *mut BlockIoMedia) -> BlockIo {
        BlockIo {
            revision: 1,
            media,
            reset: core::ptr::null_mut(),
            read_blocks: unused_read,
            write_blocks: core::ptr::null_mut(),
            flush_blocks: core::ptr::null_mut(),
        }
    }

    #[test]
    fn null_interface_is_a_firmware_violation() {
        assert_eq!(block_io_from_interface(core::ptr::null_mut()), Err(Error::Io));
    }

    #[test]
    fn null_media_is_a_firmware_violation() {
        let mut io = block_io(core::ptr::null_mut());
        let ptr = (&mut io as *mut BlockIo).cast::<c_void>();
        assert_eq!(block_io_from_interface(ptr), Err(Error::Io));
    }

    #[test]
    fn a_valid_interface_is_accepted() {
        let mut io = block_io((&MEDIA as *const BlockIoMedia).cast_mut());
        let ptr = (&mut io as *mut BlockIo).cast::<c_void>();
        let got = block_io_from_interface(ptr).expect("有效接口");
        assert_eq!(got, ptr.cast::<BlockIo>());
    }
}
