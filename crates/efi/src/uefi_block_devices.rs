//! `firmware::block::BlockDeviceSource` 的 UEFI 实现。
//!
//! 边界：设备表由调用方提供（发现阶段用 `discover_block_devices` 填充），**无隐式分配**。

use crate::types::Handle;
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

    /// 从两个固件函数指针发现并构造，**存储由本模块自己持有**。
    ///
    /// 入口不该知道 `BlockIo` 这种实现类型（边界），所以设备表放在本模块的静态里；
    /// 调用方只拿到一个可用的来源。静态表在引导阶段只初始化一次。
    ///
    /// # Safety
    ///
    /// 引导阶段单线程调用一次；重复调用会覆盖同一张静态表。
    pub unsafe fn from_boot_services(
        locate_handle: crate::boot_services_table::LocateHandle,
        handle_protocol: crate::boot_services_table::HandleProtocol,
    ) -> Result<UefiBlockDevices<'static>, Error> {
        const MAX: usize = 16;
        static mut HANDLES: [Handle; MAX] = [core::ptr::null_mut(); MAX];
        static mut DEVICES: [*mut BlockIo; MAX] = [core::ptr::null_mut(); MAX];
        // SAFETY: 由调用方保证引导阶段单线程、只调一次。
        let (handles, devices) = unsafe {
            (
                &mut *core::ptr::addr_of_mut!(HANDLES),
                &mut *core::ptr::addr_of_mut!(DEVICES),
            )
        };
        UefiBlockDevices::discover(locate_handle, handle_protocol, handles, devices)
    }

    /// 从引导服务表**发现**块设备并构造（方案 B：能力挂在已转出的类型上）。
    ///
    /// `handles` 与 `devices` 由调用方提供 —— 引导阶段不该在内核态偷偷分配。
    /// 发现逻辑复用 [`crate::discover::discover_block_devices`]（其两段式枚举已有独立测试）。
    /// 收两个固件函数指针（而不是整张表）：调用方从引导服务表取 `locate_handle` 与
    /// `handle_protocol` 传进来 —— 这样本函数可在宿主上测，无需构造整张表
    /// （`BootServicesTable` 含函数指针字段，**不允许零初始化**，`zeroed()` 是 UB）。
    pub fn discover(
        locate_handle: crate::boot_services_table::LocateHandle,
        handle_protocol: crate::boot_services_table::HandleProtocol,
        handles: &'a mut [Handle],
        devices: &'a mut [*mut BlockIo],
    ) -> Result<Self, Error> {
        let count =
            crate::discover::discover_block_devices(locate_handle, handle_protocol, handles, devices)?;
        Ok(Self { devices: &devices[..count] })
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

#[cfg(test)]
mod from_boot_services_tests {
    use super::UefiBlockDevices;
    use crate::block_io::{BlockIo, BlockIoMedia};
    use crate::types::{BUFFER_TOO_SMALL, Handle, Status, SUCCESS};
    use core::ffi::c_void;
    use core::mem::size_of;
    use firmware::block::BlockDeviceSource;

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
        _search_type: u32,
        _protocol: *const c_void,
        _key: *mut c_void,
        size: *mut usize,
        buffer: *mut Handle,
    ) -> Status {
        let unit = size_of::<Handle>();
        // SAFETY: 调用方按 UEFI 契约传入有效指针。
        unsafe {
            if buffer.is_null() {
                *size = unit;
                BUFFER_TOO_SMALL
            } else {
                *size = unit;
                *buffer = 0x10usize as *mut c_void;
                SUCCESS
            }
        }
    }

    unsafe extern "efiapi" fn fake_handle_protocol(
        _handle: Handle,
        _protocol: *const c_void,
        interface: *mut *mut c_void,
    ) -> Status {
        // SAFETY: 同上。
        unsafe { *interface = core::ptr::addr_of_mut!(IO) as *mut c_void };
        SUCCESS
    }

    #[test]
    fn the_type_owns_its_storage_so_the_caller_need_not_name_block_io() {
        // SAFETY: 测试内单线程初始化静态夹具。
        unsafe { IO.media = core::ptr::addr_of!(MEDIA) as *mut BlockIoMedia };
        // SAFETY: 引导阶段单线程调用一次；此处即测试线程。
        let source =
            unsafe { UefiBlockDevices::from_boot_services(fake_locate, fake_handle_protocol) }
                .expect("发现应成功");
        assert_eq!(source.device_count(), 1);
    }
}

#[cfg(test)]
mod discover_tests {
    use super::UefiBlockDevices;
    use crate::block_io::{BlockIo, BlockIoMedia};
    use crate::discover::SEARCH_TYPE_BY_PROTOCOL;
    use crate::types::{BUFFER_TOO_SMALL, Handle, Status, SUCCESS};
    use firmware::block::BlockDeviceSource;
    use core::ffi::c_void;
    use core::mem::size_of;

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
                BUFFER_TOO_SMALL
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
        // SAFETY: 同上；只写入调用方给的 `interface`。
        unsafe {
            let _ = MEDIA;
            *interface = core::ptr::addr_of_mut!(IO) as *mut c_void;
        }
        SUCCESS
    }

    #[test]
    fn discovery_through_the_firmware_pointers_finds_one_device() {
        // 固件契约要求 `Media` 非空（`block_io_from_interface` 会校验），夹具必须满足。
        // SAFETY: 测试内单线程初始化静态夹具。
        unsafe { IO.media = core::ptr::addr_of!(MEDIA) as *mut BlockIoMedia };
        let mut handles = [0usize as Handle; 4];
        let mut devices = [core::ptr::null_mut::<BlockIo>(); 4];
        let source =
            UefiBlockDevices::discover(fake_locate, fake_handle_protocol, &mut handles, &mut devices)
                .expect("发现应成功");
        assert_eq!(source.device_count(), 1, "假固件只暴露一个 BlockIo");
    }
}