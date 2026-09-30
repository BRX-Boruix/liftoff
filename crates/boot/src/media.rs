//! 介质胶水：把驱动层的分区视图（`driver::Volume`）适配成文件系统层需要的块读取。
//!
//! **为什么放在 `boot`（而不是 `fs`）**：`fs` 与 `driver` 是**平级**层，`fs` 不得依赖
//! `driver`；这种适配属于**编排**，归入口层。
//!
//! 边界：只依赖 `arch`/`firmware` **抽象** 与 `fs`/`driver` 的**公开接口**，不触碰固件具体实现。

use driver::volume::Volume;
use firmware::block::BlockDeviceSource;
use fs::ext2::Ext2Error;

/// 按 EXT2 的“块号”读取：把块号换算成设备 LBA，经 `Volume` 读入 `buffer`。
///
/// 约定：`buffer.len()` 就是 EXT2 块大小，且必须是 `volume.sector_size` 的整数倍。
/// 固件错误映射为 `Ext2Error::ShortImage`（`fs` 层不认识固件错误类型，故在此适配处映射）。
pub fn read_ext2_block<D: BlockDeviceSource>(
    source: &mut D,
    volume: &Volume,
    block_number: u64,
    buffer: &mut [u8],
) -> Result<(), Ext2Error> {
    let sector = volume.sector_size as u64;
    if sector == 0 {
        return Err(Ext2Error::BadBlockSize);
    }
    let block_bytes = buffer.len() as u64;
    if block_bytes == 0 || block_bytes % sector != 0 {
        return Err(Ext2Error::BadBlockSize);
    }
    let per_block = block_bytes / sector;
    let offset = block_number
        .checked_mul(per_block)
        .ok_or(Ext2Error::ShortImage)?;
    let count = u32::try_from(per_block).map_err(|_| Ext2Error::BadBlockSize)?;
    volume
        .read(source, offset, count, buffer)
        .map_err(|_| Ext2Error::ShortImage)
}
#[cfg(test)]
mod tests {
    use super::read_ext2_block;
    use driver::volume::Volume;
    use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;
    use fs::ext2::Ext2Error;

    const SECTOR: u32 = 512;

    struct Disk {
        bytes: std::vec::Vec<u8>,
    }

    impl Disk {
        fn new(sectors: u64) -> Self {
            Self { bytes: std::vec![0u8; (sectors * SECTOR as u64) as usize] }
        }
        fn info(&self) -> BlockDeviceInfo {
            BlockDeviceInfo { block_size: SECTOR, block_count: (self.bytes.len() / SECTOR as usize) as u64, read_only: true }
        }
    }

    impl BlockDeviceSource for Disk {
        fn device_count(&self) -> usize { 1 }
        fn device_info(&self, index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
            if index.0 != 0 { return Err(Error::NotFound); }
            Ok(self.info())
        }
        fn read_blocks(&mut self, index: DeviceIndex, lba: u64, count: u32, buffer: &mut [u8]) -> Result<(), Error> {
            if index.0 != 0 { return Err(Error::NotFound); }
            firmware::block::validate_read(self.info(), lba, count, buffer.len())?;
            let start = (lba * SECTOR as u64) as usize;
            let len = count as usize * SECTOR as usize;
            buffer[..len].copy_from_slice(&self.bytes[start..start + len]);
            Ok(())
        }
    }

    #[test]
    fn a_filesystem_block_maps_to_the_right_lba() {
        let mut disk = Disk::new(64);
        // 1 KiB 的 EXT2 块 = 2 个 512 字节扇区；块 2 应从 LBA 4 开始。
        disk.bytes[4 * SECTOR as usize] = 0x77;
        let volume = Volume::new(DeviceIndex(0), 0, 64, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; 1024];
        read_ext2_block(&mut disk, &volume, 2, &mut buffer).expect("读取成功");
        assert_eq!(buffer[0], 0x77, "块 2 应落在 LBA 4");
    }

    #[test]
    fn a_block_number_overflow_is_rejected() {
        let mut disk = Disk::new(64);
        let volume = Volume::new(DeviceIndex(0), 0, 64, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; 1024];
        assert_eq!(
            read_ext2_block(&mut disk, &volume, u64::MAX, &mut buffer),
            Err(Ext2Error::ShortImage)
        );
    }

    #[test]
    fn a_buffer_not_matching_the_sector_size_is_rejected() {
        let mut disk = Disk::new(64);
        let volume = Volume::new(DeviceIndex(0), 0, 64, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; 1000];
        assert_eq!(
            read_ext2_block(&mut disk, &volume, 0, &mut buffer),
            Err(Ext2Error::BadBlockSize)
        );
    }

    #[test]
    fn reading_past_the_volume_is_reported_as_a_short_image() {
        let mut disk = Disk::new(64);
        // 卷只有 4 个扇区：块 2（LBA 4、2 扇区）已越界。
        let volume = Volume::new(DeviceIndex(0), 0, 4, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; 1024];
        assert_eq!(
            read_ext2_block(&mut disk, &volume, 2, &mut buffer),
            Err(Ext2Error::ShortImage)
        );
    }
}