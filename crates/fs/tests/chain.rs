//! 端到端链路测试（集成）：**真实 EXT2 镜像** → `firmware::block` 抽象 → `driver::Volume` → EXT2 超级块。
//!
//! **如实说明本测试覆盖到哪一步**：到“解析出超级块”为止。
//! 再往下（找根目录、读文件）需要**块组描述符**来定位 inode 表，而该部分**尚未实现** —— 
//! 所以本测试**不声称**已经打通“从盘到文件”的完整链路。

use driver::volume::Volume;
use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
use firmware::error::Error;
use fs::ext2::parse_superblock;

const SECTOR: u32 = 512;

/// 假块设备：把镜像字节当成整盘。
struct Disk {
    bytes: std::vec::Vec<u8>,
}

impl BlockDeviceSource for Disk {
    fn device_count(&self) -> usize {
        1
    }

    fn device_info(&self, index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
        if index.0 != 0 {
            return Err(Error::NotFound);
        }
        Ok(BlockDeviceInfo {
            block_size: SECTOR,
            block_count: (self.bytes.len() / SECTOR as usize) as u64,
            read_only: true,
        })
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
        let info = BlockDeviceInfo {
            block_size: SECTOR,
            block_count: (self.bytes.len() / SECTOR as usize) as u64,
            read_only: true,
        };
        firmware::block::validate_read(info, lba, count, buffer.len())?;
        let start = (lba * SECTOR as u64) as usize;
        let len = count as usize * SECTOR as usize;
        buffer[..len].copy_from_slice(&self.bytes[start..start + len]);
        Ok(())
    }
}

#[test]
fn the_real_ext2_image_is_reachable_through_the_block_abstraction() {
    let bytes = std::fs::read("tests/fixtures/hello.ext2").expect("夹具镜像应存在");
    let sectors = (bytes.len() / SECTOR as usize) as u64;
    let mut disk = Disk { bytes };
    let volume = Volume::new(DeviceIndex(0), 0, sectors, SECTOR).expect("卷合法");
    // 必须**从块 0 读起**：parse_superblock 期望切片起点就是文件系统起点，
    // 它会在切片内偏移 1024 处找超级块。若从块 2 读起，切片起点已是 1024，
    // 就会读到文件偏移 2048 处（错位）。
    let mut buffer = std::vec![0u8; 2048];
    volume
        .read(&mut disk, 0, 4, &mut buffer)
        .expect("经块抽象读取成功");
    let superblock = parse_superblock(&buffer).expect("超级块可解析");
    assert_eq!(superblock.block_size, 1024);
    assert_eq!(superblock.inode_size, 128);
    assert_eq!(superblock.blocks_count, 256);
}
