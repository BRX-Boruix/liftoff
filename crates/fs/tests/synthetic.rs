//! 路线 A：**按 EXT2 规范在测试里构造**一个最小镜像，走完整链路读出普通文件内容。
//!
//! **如实标注**：镜像是我**按规范生成的**（不是 `mke2fs` 产出 —— 该环境的 `-d` 不可用）；
//! 结构真实，且各字段偏移已被 `fs::ext2` 的单元测试逐一覆盖。

use driver::volume::Volume;
use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
use firmware::error::Error;
use fs::ext2::{
    find_in_dir, group_descriptor_block, parse_group_descriptor, parse_inode, parse_superblock,
    read_file,
};

const SECTOR: u32 = 512;
const BLOCK: usize = 1024;
const BLOCKS: usize = 16;

struct Disk {
    bytes: std::vec::Vec<u8>,
}

impl Disk {
    fn info(&self) -> BlockDeviceInfo {
        BlockDeviceInfo {
            block_size: SECTOR,
            block_count: (self.bytes.len() / SECTOR as usize) as u64,
            read_only: true,
        }
    }
}

impl BlockDeviceSource for Disk {
    fn device_count(&self) -> usize {
        1
    }

    fn device_info(&self, index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
        if index.0 != 0 {
            return Err(Error::NotFound);
        }
        Ok(self.info())
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
        firmware::block::validate_read(self.info(), lba, count, buffer.len())?;
        let start = (lba * SECTOR as u64) as usize;
        let len = count as usize * SECTOR as usize;
        buffer[..len].copy_from_slice(&self.bytes[start..start + len]);
        Ok(())
    }
}

/// 写一个 u16 / u32（小端）。
fn put16(target: &mut [u8], at: usize, value: u16) {
    target[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(target: &mut [u8], at: usize, value: u32) {
    target[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// 构造最小 EXT2 镜像（1 KiB 块）。
/// 布局：块 1 内偏移 1024 = 超级块；块 2 = 块组描述符；块 3 = inode 表；
/// 块 4 = 根目录数据；块 5 = HELLO.TXT 数据。
fn image() -> std::vec::Vec<u8> {
    let mut bytes = std::vec![0u8; BLOCKS * BLOCK];
    // 超级块（文件系统内偏移 1024）。
    let sb = 1024;
    put32(&mut bytes, sb, 16);
    put32(&mut bytes, sb + 4, BLOCKS as u32);
    put32(&mut bytes, sb + 20, 1);
    put32(&mut bytes, sb + 24, 0);
    put32(&mut bytes, sb + 40, 16);
    put16(&mut bytes, sb + 56, 0xEF53);
    put32(&mut bytes, sb + 76, 1);
    put32(&mut bytes, sb + 84, 11);
    put16(&mut bytes, sb + 88, 128);
    // 块组描述符表（块 2）：inode 表在块 3（偏移 8）。
    put32(&mut bytes, 2 * BLOCK + 8, 3);
    // inode 表（块 3）：根目录 = inode 2，文件 = inode 11。
    let table = 3 * BLOCK;
    let root = table + 128;
    put16(&mut bytes, root, 0x41ED);
    put32(&mut bytes, root + 4, BLOCK as u32);
    put32(&mut bytes, root + 40, 4);
    let file = table + 128 * 10;
    put16(&mut bytes, file, 0x81A4);
    put32(&mut bytes, file + 4, 7);
    put32(&mut bytes, file + 40, 5);
    // 根目录数据（块 4）：. / .. / HELLO.TXT
    let dir = 4 * BLOCK;
    bytes[dir] = 12;
    put32(&mut bytes, dir, 2);
    put16(&mut bytes, dir + 4, 12);
    bytes[dir + 6] = 1;
    bytes[dir + 7] = 2;
    bytes[dir + 8] = 0x00;
    let second = dir + 12;
    put32(&mut bytes, second, 2);
    put16(&mut bytes, second + 4, 12);
    bytes[second + 6] = 1;
    bytes[second + 7] = 2;
    bytes[second + 8] = 0x01;
    let third = dir + 24;
    let name = b"HELLO.TXT";
    let record = 8 + name.len();
    put32(&mut bytes, third, 11);
    put16(&mut bytes, third + 4, (BLOCK - 24) as u16);
    bytes[third + 6] = name.len() as u8;
    bytes[third + 7] = 1;
    bytes[third + 8..third + 8 + name.len()].copy_from_slice(name);
    let _ = record;
    // 文件数据（块 5）。
    bytes[5 * BLOCK..5 * BLOCK + 7].copy_from_slice(b"liftoff");
    bytes
}

#[test]
fn a_regular_file_is_found_and_read_through_the_whole_chain() {
    let bytes = image();
    let sectors = (bytes.len() / SECTOR as usize) as u64;
    let mut disk = Disk { bytes };
    let volume = Volume::new(DeviceIndex(0), 0, sectors, SECTOR).expect("卷合法");
    let per_block = (BLOCK as u64 / SECTOR as u64) as u32;

    // 超级块：从块 0 读（切片起点即文件系统起点）。
    let mut head = std::vec![0u8; 2 * BLOCK];
    volume.read(&mut disk, 0, 2 * per_block, &mut head).expect("读超级块");
    let superblock = parse_superblock(&head).expect("超级块可解析");
    assert_eq!(superblock.block_size, 1024);
    assert_eq!(superblock.inode_size, 128);

    // 块组描述符 → inode 表位置。
    let gd_block = group_descriptor_block(superblock.block_size).expect("描述符表块号");
    let mut gd = std::vec![0u8; BLOCK];
    volume
        .read(&mut disk, gd_block * per_block as u64, per_block, &mut gd)
        .expect("读描述符表");
    let group = parse_group_descriptor(&gd, 0).expect("解析描述符");
    assert_eq!(group.inode_table, 3);

    // inode 表：一次读入足够放 inode 11 的块数（这里只读第一块）。
    // inode 表每块只放 BLOCK / inode_size = 8 个 inode：inode 11 在**第二块**，
    // 故必须一次读入两块（曾经只读一块 → ShortImage）。
    let mut table = std::vec![0u8; 2 * BLOCK];
    volume
        .read(&mut disk, group.inode_table as u64 * per_block as u64, 2 * per_block, &mut table)
        .expect("读 inode 表");
    let root = parse_inode(&table, 2, superblock.inode_size).expect("根目录 inode");
    assert_eq!(root.blocks[0], 4);

    // 根目录数据块 → 找到 HELLO.TXT。
    let mut dir = std::vec![0u8; BLOCK];
    volume
        .read(&mut disk, root.blocks[0] as u64 * per_block as u64, per_block, &mut dir)
        .expect("读根目录块");
    let entry = find_in_dir(&dir, b"HELLO.TXT").expect("查找成功").expect("应找到 HELLO.TXT");
    assert_eq!(entry.inode, 11);

    // 文件 inode → 读出内容。
    let inode = parse_inode(&table, entry.inode, superblock.inode_size).expect("文件 inode");
    assert_eq!(inode.size, 7);
    assert_eq!(inode.blocks[0], 5);
    let mut scratch = std::vec![0u8; BLOCK];
    let mut content = std::vec![0u8; 7];
    let mut device = disk;
    let read = read_file(&inode, 1024, &mut scratch, &mut content, |number, buffer| {
        let lba = number as u64 * per_block as u64;
        // 把固件错误映射为 EXT2 层的错误（两层错误类型不同，不做有损扁平化，
        // 这里只在本测试的适配闭包里映射）。
        device
            .read_blocks(DeviceIndex(0), lba, per_block, buffer)
            .map_err(|_| fs::ext2::Ext2Error::ShortImage)
    })
    .expect("读取文件内容");
    assert_eq!(read, 7);
    assert_eq!(&content[..], b"liftoff", "读出的内容必须与写入的一致");
}
