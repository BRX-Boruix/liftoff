//! `firmware::block::BlockDeviceSource` 的 UEFI 实现。
//!
//! 边界：设备表由调用方提供（发现阶段用 `discover_block_devices` 填充），**无隐式分配**。

use crate::block_io::{BlockIo, BlockIoMedia, media_to_info};
use crate::block_source::read_once;
use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
use firmware::error::Error;

/// 基于已发现设备表的块设备来源。
pub struct UefiBlockDevices<'a> {
    devices: &'a [*mut BlockIo],
}

impl<'a> UefiBlockDevices<'a> {
    /// 以设备表构造。
    pub const fn new(devices: &'a [*mut BlockIo]) -> Self {
        Self { devices }
    }

    fn device(&self, index: DeviceIndex) -> Result<*mut BlockIo, Error> {
        self.devices
            .get(index.0 as usize)
            .copied()
            .ok_or(Error::NotFound)
    }

    fn media(&self, index: DeviceIndex) -> Result<(*mut BlockIo, &BlockIoMedia), Error> {
        let device = self.device(index)?;
        // SAFETY: 设备表由发现阶段填充，每项非空且指向有效 `BlockIo`（见 `discover`）。
        let media = unsafe { (*device).media };
        if media.is_null() {
            return Err(Error::Io);
        }
        // SAFETY: `media` 非空（上面已判），由固件保证指向有效介质描述。
        Ok((device, unsafe { &*media }))
    }
}

impl BlockDeviceSource for UefiBlockDevices<'_> {
    fn device_count(&self) -> usize {
        self.devices.len()
    }

    fn device_info(&self, index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
        let (_, media) = self.media(index)?;
        media_to_info(media)
    }

    fn read_blocks(
        &mut self,
        index: DeviceIndex,
        lba: u64,
        count: u32,
        buffer: &mut [u8],
    ) -> Result<(), Error> {
        let (device, media) = self.media(index)?;
        let info = media_to_info(media)?;
        let media_id = media.media_id;
        // SAFETY: `device` 指向有效 `BlockIo`（见 `media` 的说明），其 `read_blocks` 由固件提供。
        let read_blocks = unsafe { (*device).read_blocks };
        read_once(read_blocks, device, media_id, info, lba, count, buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::UefiBlockDevices;
    use crate::block_io::{BlockIo, BlockIoMedia};
    use crate::types::{Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::block::{BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;

    static SEEN_BYTES: AtomicUsize = AtomicUsize::new(0);

    static MEDIA: BlockIoMedia = BlockIoMedia {
        media_id: 0x42,
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

    unsafe extern "efiapi" fn fake_read(
        _this: *mut BlockIo,
        _media_id: u32,
        _lba: u64,
        buffer_size: usize,
        buffer: *mut c_void,
    ) -> Status {
        SEEN_BYTES.store(buffer_size, Ordering::SeqCst);
        // SAFETY: 调用方按契约传入至少 buffer_size 字节的可写缓冲。
        unsafe { core::ptr::write_bytes(buffer.cast::<u8>(), 0x5A, buffer_size) };
        SUCCESS
    }

    static mut IO: BlockIo = BlockIo {
        revision: 1,
        media: core::ptr::null_mut(),
        reset: core::ptr::null_mut(),
        read_blocks: fake_read,
        write_blocks: core::ptr::null_mut(),
        flush_blocks: core::ptr::null_mut(),
    };

    fn devices() -> [*mut BlockIo; 1] {
        // SAFETY: 测试内单线程；写入有效指针，不创建静态引用。
        unsafe {
            (*core::ptr::addr_of_mut!(IO)).media = (&MEDIA as *const BlockIoMedia).cast_mut();
            [core::ptr::addr_of_mut!(IO)]
        }
    }

    #[test]
    fn device_count_and_info_come_from_the_table() {
        let table = devices();
        let source = UefiBlockDevices::new(&table);
        assert_eq!(source.device_count(), 1);
        let info = source.device_info(DeviceIndex(0)).expect("设备信息");
        assert_eq!(info.block_size, 512);
        assert_eq!(info.block_count, 8, "LastBlock 含尾：7 + 1");
        assert_eq!(source.device_info(DeviceIndex(1)), Err(Error::NotFound));
    }

    #[test]
    fn read_blocks_delegates_to_the_firmware_call() {
        let table = devices();
        let mut source = UefiBlockDevices::new(&table);
        let mut buffer = [0u8; 1024];
        source.read_blocks(DeviceIndex(0), 0, 2, &mut buffer).expect("读成功");
        assert_eq!(SEEN_BYTES.load(Ordering::SeqCst), 1024, "必须传字节数");
        assert_eq!(buffer[0], 0x5A);
        assert_eq!(source.read_blocks(DeviceIndex(9), 0, 1, &mut buffer), Err(Error::NotFound));
    }
}
