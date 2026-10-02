//! 卷：把分区表与块设备接起来，提供“按分区读块”（L4）。
//!
//! 边界：只认 `firmware::block::BlockDeviceSource` **抽象**（具体实现由门面注入）；
//! 边界校验**复用** `firmware::block::validate_read`（单点定义，不写第二份）。
//!
//! 约定：**被拒绝的读绝不触碰设备**（先判后调）—— 越界访问在引导器里是不可接受的。

use firmware::block::{BlockDeviceSource, DeviceIndex};
use firmware::error::Error;

/// 一个分区视图。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Volume {
    /// 所属块设备。
    pub device: DeviceIndex,
    /// 分区起始 LBA（设备内绝对块号）。
    pub start_lba: u64,
    /// 分区扇区数。
    pub sector_count: u64,
    /// 扇区大小（字节）。
    pub sector_size: u32,
}

impl Volume {
    /// 构造分区视图；扇区大小为 0 或分区为空视为调用方错误。
    pub fn new(
        device: DeviceIndex,
        start_lba: u64,
        sector_count: u64,
        sector_size: u32,
    ) -> Result<Self, Error> {
        if sector_size == 0 || sector_count == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(Self { device, start_lba, sector_count, sector_size })
    }

    /// 读分区内的块；`offset` 是**相对分区起点**的块号。
    pub fn read<D: BlockDeviceSource>(
        &self,
        source: &mut D,
        offset: u64,
        count: u32,
        buffer: &mut [u8],
    ) -> Result<(), Error> {
        if count == 0 {
            return Err(Error::InvalidArgument);
        }
        // 先做**分区内**边界检查（checked，绝不回绕）。
        let end = offset.checked_add(count as u64).ok_or(Error::InvalidArgument)?;
        if end > self.sector_count {
            return Err(Error::InvalidArgument);
        }
        // 再换算成设备内的绝对 LBA。
        let lba = self.start_lba.checked_add(offset).ok_or(Error::InvalidArgument)?;
        // 设备自身的边界由 `read_blocks` 内部按 `validate_read` 再查一次。
        source.read_blocks(self.device, lba, count, buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::Volume;
    use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;
    use std::vec::Vec;

    const SECTOR: u32 = 512;

    /// 假块设备：整盘字节 + 信息。
    struct FakeDisk {
        bytes: Vec<u8>,
        info: BlockDeviceInfo,
        reads: usize,
    }

    impl FakeDisk {
        fn new(sectors: u64) -> Self {
            Self {
                bytes: std::vec![0u8; (sectors * SECTOR as u64) as usize],
                info: BlockDeviceInfo { block_size: SECTOR, block_count: sectors, read_only: true },
                reads: 0,
            }
        }
    }

    impl BlockDeviceSource for FakeDisk {
        fn device_count(&self) -> usize {
            1
        }

        fn device_info(&self, index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
            if index.0 != 0 {
                return Err(Error::NotFound);
            }
            Ok(self.info)
        }

        fn read_blocks(
            &mut self,
            index: DeviceIndex,
            lba: u64,
            count: u32,
            buffer: &mut [u8],
        ) -> Result<(), Error> {
            if index.0 != 0 {
                return Err(Error::NotFound);
            }
            firmware::block::validate_read(self.info, lba, count, buffer.len())?;
            self.reads += 1;
            let start = (lba * SECTOR as u64) as usize;
            let len = count as usize * SECTOR as usize;
            buffer[..len].copy_from_slice(&self.bytes[start..start + len]);
            Ok(())
        }
    }

    #[test]
    fn reading_a_volume_starts_at_the_partition_lba() {
        let mut disk = FakeDisk::new(64);
        // 分区从 LBA 8 开始；在 LBA 8 处写一个可识别字节。
        disk.bytes[8 * SECTOR as usize] = 0x5A;
        let volume = Volume::new(DeviceIndex(0), 8, 16, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; SECTOR as usize];
        volume.read(&mut disk, 0, 1, &mut buffer).expect("读取成功");
        assert_eq!(buffer[0], 0x5A, "偏移 0 应落在分区起始 LBA");
        assert_eq!(disk.reads, 1);
    }

    #[test]
    fn reading_past_the_partition_end_is_rejected_without_touching_the_disk() {
        let mut disk = FakeDisk::new(64);
        let volume = Volume::new(DeviceIndex(0), 8, 16, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; SECTOR as usize];
        // 分区只有 16 个扇区：偏移 16 已越界。
        assert_eq!(
            volume.read(&mut disk, 16, 1, &mut buffer),
            Err(Error::InvalidArgument)
        );
        assert_eq!(disk.reads, 0, "越界不得触碰设备");
    }

    #[test]
    fn a_zero_sector_size_is_rejected() {
        assert_eq!(
            Volume::new(DeviceIndex(0), 0, 16, 0).map(|_| ()),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn an_empty_partition_is_rejected() {
        assert_eq!(
            Volume::new(DeviceIndex(0), 0, 0, SECTOR).map(|_| ()),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn an_lba_overflow_is_rejected() {
        let mut disk = FakeDisk::new(64);
        // 分区起点极大：start_lba + offset 会溢出。
        let volume = Volume::new(DeviceIndex(0), u64::MAX - 1, 16, SECTOR).expect("卷合法");
        let mut buffer = std::vec![0u8; SECTOR as usize];
        assert_eq!(
            volume.read(&mut disk, 8, 1, &mut buffer),
            Err(Error::InvalidArgument)
        );
    }
}