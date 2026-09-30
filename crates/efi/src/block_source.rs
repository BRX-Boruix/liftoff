//! 块设备读块的编排。
//!
//! 边界：只做**一次读的编排**（校验 → 调用固件 → 映射返回码）；设备枚举与协议查找在
//! `BlockDeviceSource` 的实现里完成。

use crate::block_io::{BlockIo, ReadBlocks};
use crate::block_read::read_byte_len;
use crate::status::status_to_error;
use core::ffi::c_void;
use firmware::block::{BlockDeviceInfo, validate_read};
use firmware::error::Error;

/// 读一次块：先按抽象契约校验参数（**不合法就不碰固件**），再调用固件，最后映射返回码。
pub fn read_once(
    read_blocks: ReadBlocks,
    this: *mut BlockIo,
    media_id: u32,
    info: BlockDeviceInfo,
    lba: u64,
    count: u32,
    buffer: &mut [u8],
) -> Result<(), Error> {
    validate_read(info, lba, count, buffer.len())?;
    let bytes = read_byte_len(count, info.block_size);
    // SAFETY: `this` 由调用方保证是有效的 `BlockIo` 指针；`buffer` 至少有 `bytes` 字节
    // （`validate_read` 已按块大小校验过容量），且 `bytes` 由 u32 乘积得出、不会溢出。
    let status = unsafe {
        read_blocks(this, media_id, lba, bytes, buffer.as_mut_ptr().cast::<c_void>())
    };
    match status_to_error(status) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::read_once;
    use crate::block_io::BlockIo;
    use crate::types::{DEVICE_ERROR, Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::block::BlockDeviceInfo;
    use firmware::error::Error;

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static SEEN_BYTES: AtomicUsize = AtomicUsize::new(0);
    static FAIL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_read(
        _this: *mut BlockIo,
        _media_id: u32,
        _lba: u64,
        buffer_size: usize,
        buffer: *mut c_void,
    ) -> Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        SEEN_BYTES.store(buffer_size, Ordering::SeqCst);
        if FAIL.load(Ordering::SeqCst) == 1 {
            return DEVICE_ERROR;
        }
        // SAFETY: 调用方按契约传入至少 buffer_size 字节的可写缓冲；测试中始终如此。
        unsafe { core::ptr::write_bytes(buffer.cast::<u8>(), 0xAB, buffer_size) };
        SUCCESS
    }

    fn info() -> BlockDeviceInfo {
        BlockDeviceInfo { block_size: 512, block_count: 8, read_only: false }
    }

    #[test]
    fn valid_read_passes_the_byte_size_and_fills_the_buffer() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        let mut buffer = [0u8; 1024];
        read_once(fake_read, core::ptr::null_mut(), 7, info(), 0, 2, &mut buffer).expect("读成功");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(SEEN_BYTES.load(Ordering::SeqCst), 1024, "必须传字节数而非块数");
        assert_eq!(buffer[0], 0xAB);
    }

    #[test]
    fn invalid_arguments_are_rejected_without_calling_firmware() {
        CALLS.store(0, Ordering::SeqCst);
        let mut buffer = [0u8; 1024];
        assert_eq!(read_once(fake_read, core::ptr::null_mut(), 7, info(), 8, 1, &mut buffer), Err(Error::InvalidArgument));
        assert_eq!(read_once(fake_read, core::ptr::null_mut(), 7, info(), 0, 3, &mut buffer), Err(Error::BufferTooSmall));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "参数非法时不得调用固件");
    }

    #[test]
    fn firmware_errors_are_mapped() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(1, Ordering::SeqCst);
        let mut buffer = [0u8; 1024];
        assert_eq!(read_once(fake_read, core::ptr::null_mut(), 7, info(), 0, 1, &mut buffer), Err(Error::Io));
    }
}
