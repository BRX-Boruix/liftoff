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

/// 把链路再推进一步：经真实镜像**列出根目录**。
///
/// **如实说明**：夹具镜像由 `mke2fs` 生成时**未能填充文件**（该环境的 `-d` 不可用），
/// 故根目录里只应有 `mke2fs` 必然创建的 `lost+found`。本测试**只断言这一点**，
/// **不声称**读到普通文件。
#[test]
fn the_root_directory_of_the_real_image_is_listed() {
    use fs::ext2::{
        DirEntry, group_descriptor_block, parse_dir_entries, parse_group_descriptor, parse_inode,
    };

    let bytes = std::fs::read("tests/fixtures/hello.ext2").expect("夹具镜像应存在");
    let sectors = (bytes.len() / SECTOR as usize) as u64;
    let mut disk = Disk { bytes };
    let volume = Volume::new(DeviceIndex(0), 0, sectors, SECTOR).expect("卷合法");

    // 1) 超级块：必须从块 0 读（切片起点即文件系统起点）。
    let mut head = std::vec![0u8; 2048];
    volume.read(&mut disk, 0, 4, &mut head).expect("读超级块");
    let superblock = parse_superblock(&head).expect("超级块可解析");
    let block_size = superblock.block_size;
    assert_eq!(block_size, 1024);
    let sectors_per_block = (block_size / SECTOR) as u64;
    let block_bytes = block_size as usize;

    // 2) 块组描述符表 → inode 表位置。
    let gd_block = group_descriptor_block(block_size).expect("描述符表块号");
    let mut gd = std::vec![0u8; block_bytes];
    volume
        .read(&mut disk, gd_block * sectors_per_block, sectors_per_block as u32, &mut gd)
        .expect("读描述符表");
    let group = parse_group_descriptor(&gd, 0).expect("解析描述符");

    // 3) inode 表 → 根目录 inode（EXT2 里根目录固定是 inode 2）。
    let mut table = std::vec![0u8; block_bytes];
    volume
        .read(&mut disk, group.inode_table as u64 * sectors_per_block, sectors_per_block as u32, &mut table)
        .expect("读 inode 表");
    let root = parse_inode(&table, 2, superblock.inode_size).expect("根目录 inode");

    // 4) 根目录数据块 → 目录项。
    let mut dir = std::vec![0u8; block_bytes];
    volume
        .read(&mut disk, root.blocks[0] as u64 * sectors_per_block, sectors_per_block as u32, &mut dir)
        .expect("读根目录块");
    let mut entries = [DirEntry::EMPTY; 16];
    let count = parse_dir_entries(&dir, &mut entries).expect("解析目录");
    assert!(count >= 2, "至少应有 . 与 ..，实际 {count}");
    let names: std::vec::Vec<&[u8]> = entries[..count].iter().map(|entry| entry.name()).collect();
    assert!(
        names.iter().any(|name| *name == b"lost+found"),
        "mke2fs 必然创建 lost+found，实际目录项: {names:?}"
    );
}
