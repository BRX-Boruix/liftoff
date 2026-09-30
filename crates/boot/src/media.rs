//! 介质胶水：把驱动层的分区视图（`driver::Volume`）适配成文件系统层需要的块读取。
//!
//! **为什么放在 `boot`（而不是 `fs`）**：`fs` 与 `driver` 是**平级**层，`fs` 不得依赖
//! `driver`；这种适配属于**编排**，归入口层。
//!
//! 边界：只依赖 `arch`/`firmware` **抽象** 与 `fs`/`driver` 的**公开接口**，不触碰固件具体实现。

use driver::partition::Partition;
use firmware::error::Error;
use driver::table::{TableError, parse_partition_table};
use firmware::block::DeviceIndex;
use fs::iso9660::{ISO9660_DESCRIPTOR_OFFSET, ISO9660_DESCRIPTOR_SIZE, IsoError, find_in_directory, parse_primary_descriptor};
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

#[cfg(test)]
mod chain_tests {
    use super::read_ext2_block;
    use driver::volume::Volume;
    use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;
    use fs::ext2::{
        DirEntry, group_descriptor_block, parse_dir_entries, parse_group_descriptor, parse_inode,
        parse_superblock,
    };

    const SECTOR: u32 = 512;
    const BLOCK: usize = 1024;

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

    /// 经**生产路径**（`boot::media::read_ext2_block`）走真实镜像直到根目录。
    ///
    /// 放在 lib 的测试模块里（而非 `tests/` 目录）：因为 `cargo test` 会构建 UEFI 的 bin，
    /// 而它在宿主上无法链接；放在 lib 测试里也**不需要 dev-dependencies**。
    #[test]
    fn the_real_image_is_walked_through_the_production_glue() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../fs/tests/fixtures/hello.ext2");
        let bytes = std::fs::read(path).expect("夹具镜像应存在");
        let sectors = (bytes.len() / SECTOR as usize) as u64;
        let mut disk = Disk { bytes };
        let volume = Volume::new(DeviceIndex(0), 0, sectors, SECTOR).expect("卷合法");

        let mut head = std::vec![0u8; 2 * BLOCK];
        read_ext2_block(&mut disk, &volume, 0, &mut head).expect("经生产胶水读头部");
        let superblock = parse_superblock(&head).expect("超级块可解析");
        assert_eq!(superblock.block_size, 1024);
        assert_eq!(superblock.inode_size, 128);
        assert_eq!(superblock.blocks_count, 256);

        let gd_block = group_descriptor_block(superblock.block_size).expect("描述符表块号");
        let mut gd = std::vec![0u8; BLOCK];
        read_ext2_block(&mut disk, &volume, gd_block, &mut gd).expect("经生产胶水读描述符");
        let group = parse_group_descriptor(&gd, 0).expect("解析描述符");

        let mut table = std::vec![0u8; BLOCK];
        read_ext2_block(&mut disk, &volume, group.inode_table as u64, &mut table)
            .expect("经生产胶水读 inode 表");
        let root = parse_inode(&table, 2, superblock.inode_size).expect("根目录 inode");
        let mut dir = std::vec![0u8; BLOCK];
        read_ext2_block(&mut disk, &volume, root.blocks[0] as u64, &mut dir)
            .expect("经生产胶水读根目录块");
        let mut entries = [DirEntry::EMPTY; 16];
        let count = parse_dir_entries(&dir, &mut entries).expect("解析目录");
        let names: std::vec::Vec<&[u8]> = entries[..count].iter().map(|entry| entry.name()).collect();
        assert!(
            names.iter().any(|name| *name == b"lost+found"),
            "经生产路径应列出 lost+found，实际: {names:?}"
        );
    }
}

/// 读 ISO9660 的**逻辑块**到缓冲（逻辑块通常 2048 字节，而设备按扇区寻址）。
///
/// 换算：`logical_block × (block_size / sector_size)` = **分区内**扇区偏移。
/// **先判后调**：块大小为 0、不是扇区整数倍、缓冲不足、或换算后越出分区，都不碰设备。
///
/// 固件错误映射为 `IsoError::ShortImage`（`fs` 层不认识固件错误类型，故在此适配处映射 ——
/// 与 `read_ext2_block` 的约定一致，不另立一档）。
pub fn read_iso_block<D: BlockDeviceSource>(
    source: &mut D,
    volume: &Volume,
    block_size: u16,
    logical_block: u32,
    buffer: &mut [u8],
) -> Result<(), IsoError> {
    let size = block_size as u32;
    if size == 0 {
        return Err(IsoError::BadBlockSize);
    }
    let sector = volume.sector_size;
    if sector == 0 || size % sector != 0 {
        return Err(IsoError::BadBlockSize);
    }
    if buffer.len() < size as usize {
        return Err(IsoError::BufferTooSmall);
    }
    let per_block = size / sector;
    let offset = (logical_block as u64)
        .checked_mul(per_block as u64)
        .ok_or(IsoError::ShortImage)?;
    volume
        .read(source, offset, per_block, &mut buffer[..size as usize])
        .map_err(|_| IsoError::ShortImage)
}

#[cfg(test)]
mod iso_block_tests {
    use super::read_iso_block;
    use driver::volume::Volume;
    use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;
    use fs::iso9660::{find_in_directory, parse_primary_descriptor};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::vec::Vec;

    static READS: AtomicUsize = AtomicUsize::new(0);

    /// 假块设备：整盘就是给定字节（扇区 512）。
    struct FakeDisk {
        bytes: Vec<u8>,
    }

    impl BlockDeviceSource for FakeDisk {
        fn device_count(&self) -> usize {
            1
        }
        fn device_info(&self, _index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
            Ok(BlockDeviceInfo {
                block_size: 512,
                block_count: (self.bytes.len() / 512) as u64,
                read_only: true,
            })
        }
        fn read_blocks(
            &mut self,
            _index: DeviceIndex,
            lba: u64,
            count: u32,
            buffer: &mut [u8],
        ) -> Result<(), Error> {
            READS.fetch_add(1, Ordering::SeqCst);
            let at = (lba as usize).checked_mul(512).ok_or(Error::InvalidArgument)?;
            let len = (count as usize) * 512;
            let src = self.bytes.get(at..at + len).ok_or(Error::InvalidArgument)?;
            buffer
                .get_mut(..len)
                .ok_or(Error::InvalidArgument)?
                .copy_from_slice(src);
            Ok(())
        }
    }

    fn whole_volume(disk: &FakeDisk) -> Volume {
        Volume::new(DeviceIndex(0), 0, (disk.bytes.len() / 512) as u64, 512).expect("卷可建")
    }

    /// 用**真实 ISO** 验证：定位内核 extent 后，把它读出来应当是 ELF。
    #[test]
    fn the_kernel_extent_of_the_real_iso_reads_back_as_an_elf() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let Ok(image) = std::fs::read(path) else {
            std::eprintln!("跳过：真实 ISO 不存在（{path}）");
            return;
        };
        let descriptor = parse_primary_descriptor(&image).expect("PVD 可解析");
        let mut disk = FakeDisk { bytes: image };
        let volume = whole_volume(&disk);
        let mut block = std::vec![0u8; 2048];

        // 先定位：根 -> BOOT -> KERNEL.;1（读块走 read_iso_block，与生产路径同一条）。
        let boot = {
            let mut read = |lba: u32, out: &mut [u8]| {
                read_iso_block(&mut disk, &volume, 2048, lba, out)
            };
            find_in_directory(
                descriptor.root.extent_lba,
                descriptor.root.data_length,
                descriptor.block_size,
                b"boot",
                &mut block,
                &mut read,
            )
            .expect("根目录可查")
            .expect("必须有 BOOT")
        };
        assert_eq!(boot.flags & 2, 2, "BOOT 是目录");

        let kernel = {
            let mut read = |lba: u32, out: &mut [u8]| read_iso_block(&mut disk, &volume, 2048, lba, out);
            find_in_directory(
                boot.extent_lba,
                boot.data_length,
                descriptor.block_size,
                b"kernel",
                &mut block,
                &mut read,
            )
            .expect("BOOT 可查")
            .expect("必须有内核")
        };
        assert_eq!(kernel.extent_lba, 33);

        // 把内核的第一个逻辑块读出来，应当是 ELF 头。
        let mut first = std::vec![0u8; 2048];
        read_iso_block(&mut disk, &volume, 2048, kernel.extent_lba, &mut first).expect("可读");
        assert_eq!(&first[..4], b"\x7fELF", "内核 extent 处必须是 ELF");
    }

    #[test]
    fn a_block_size_that_is_not_a_sector_multiple_is_rejected_without_touching_the_device() {
        let disk = FakeDisk { bytes: std::vec![0u8; 512 * 64] };
        let volume = whole_volume(&disk);
        let mut out = std::vec![0u8; 1000];
        READS.store(0, Ordering::SeqCst);
        let mut disk = disk;
        let result = read_iso_block(&mut disk, &volume, 1000, 0, &mut out);
        assert_eq!(result, Err(fs::iso9660::IsoError::BadBlockSize));
        assert_eq!(READS.load(Ordering::SeqCst), 0, "块大小不合法时绝不能碰设备");
    }
}

/// 从 ISO 上把内核映像读进 `out`，返回实际长度。
///
/// 路径固定为 `/boot/kernel`（在盘上的实际形态是 `BOOT/KERNEL.;1`，见台账的实测记录）。
/// 读块一律走 [`read_iso_block`]，与生产路径同一条。
///
/// `head` 至少要 `32768 + 2048` 字节：`parse_primary_descriptor` 按**整个映像**的偏移定位
/// PVD，而这里是按块读的，故先把覆盖 PVD 的前若干逻辑块读进来交给它。
///
/// 缓冲装不下内核时**报错而不是截断** —— 截断的内核仍是合法 ELF，跳进去才在别处炸。
pub fn read_kernel_image<D: BlockDeviceSource>(
    source: &mut D,
    volume: &Volume,
    block_size: u16,
    head: &mut [u8],
    out: &mut [u8],
) -> Result<usize, IsoError> {
    let size = block_size as usize;
    if size == 0 {
        return Err(IsoError::BadBlockSize);
    }
    let need_head = ISO9660_DESCRIPTOR_OFFSET + ISO9660_DESCRIPTOR_SIZE;
    if head.len() < need_head {
        return Err(IsoError::BufferTooSmall);
    }
    let head_blocks = need_head.div_ceil(size);
    for index in 0..head_blocks {
        let at = index * size;
        read_iso_block(source, volume, block_size, index as u32, &mut head[at..at + size])?;
    }
    let descriptor = parse_primary_descriptor(head)?;
    let mut block = [0u8; ISO9660_DESCRIPTOR_SIZE];
    let boot = find_in_directory(
        descriptor.root.extent_lba,
        descriptor.root.data_length,
        descriptor.block_size,
        b"boot",
        &mut block,
        |lba, buf| read_iso_block(source, volume, block_size, lba, buf),
    )?
    .ok_or(IsoError::ShortImage)?;
    let kernel = find_in_directory(
        boot.extent_lba,
        boot.data_length,
        descriptor.block_size,
        b"kernel",
        &mut block,
        |lba, buf| read_iso_block(source, volume, block_size, lba, buf),
    )?
    .ok_or(IsoError::ShortImage)?;
    let len = kernel.data_length as usize;
    if out.len() < len {
        return Err(IsoError::BufferTooSmall);
    }
    let file_blocks = len.div_ceil(size);
    for index in 0..file_blocks {
        read_iso_block(source, volume, block_size, kernel.extent_lba + index as u32, &mut block)?;
        let at = index * size;
        let take = core::cmp::min(size, len - at);
        out[at..at + take].copy_from_slice(&block[..take]);
    }
    Ok(len)
}

#[cfg(test)]
mod read_kernel_image_tests {
    use super::read_kernel_image;
    use driver::volume::Volume;
    use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;
    use std::vec::Vec;

    /// 假块设备：整盘就是给定字节（扇区 512）。
    struct FakeDisk {
        bytes: Vec<u8>,
    }

    impl BlockDeviceSource for FakeDisk {
        fn device_count(&self) -> usize {
            1
        }
        fn device_info(&self, _index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
            Ok(BlockDeviceInfo {
                block_size: 512,
                block_count: (self.bytes.len() / 512) as u64,
                read_only: true,
            })
        }
        fn read_blocks(
            &mut self,
            _index: DeviceIndex,
            lba: u64,
            count: u32,
            buffer: &mut [u8],
        ) -> Result<(), Error> {
            let at = (lba as usize).checked_mul(512).ok_or(Error::InvalidArgument)?;
            let len = (count as usize) * 512;
            let src = self.bytes.get(at..at + len).ok_or(Error::InvalidArgument)?;
            buffer
                .get_mut(..len)
                .ok_or(Error::InvalidArgument)?
                .copy_from_slice(src);
            Ok(())
        }
    }

    #[test]
    fn the_real_iso_yields_the_kernel_image_bytes() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let Ok(bytes) = std::fs::read(path) else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut disk = FakeDisk { bytes };
        let volume = Volume::new(DeviceIndex(0), 0, (disk.bytes.len() / 512) as u64, 512)
            .expect("卷可建");
        // PVD 在偏移 32768，故头缓冲至少要覆盖 32768 + 2048。
        let mut head = std::vec![0u8; 17 * 2048];
        let mut out = std::vec![0u8; 24_619_400];
        let len = read_kernel_image(&mut disk, &volume, 2048, &mut head, &mut out)
            .expect("内核可读出");
        assert_eq!(len, 24_619_400, "实测内核大小");
        assert_eq!(&out[..4], b"\x7fELF", "读出来的必须是 ELF");
        // 交叉验证：读出的字节应与 ISO 里 extent 33 处的字节一致。
        let at = 33 * 2048;
        assert_eq!(&out[..64], &disk.bytes[at..at + 64]);
    }

    #[test]
    fn a_buffer_too_small_for_the_kernel_is_rejected() {
        let mut disk = FakeDisk { bytes: std::vec![0u8; 512 * 64] };
        let volume = Volume::new(DeviceIndex(0), 0, (disk.bytes.len() / 512) as u64, 512)
            .expect("卷可建");
        let mut head = std::vec![0u8; 17 * 2048];
        let mut out = std::vec![0u8; 16];
        let result = read_kernel_image(&mut disk, &volume, 2048, &mut head, &mut out);
        assert!(result.is_err(), "缓冲装不下内核必须报错，不能截断交付");
    }
}

/// 介质访问失败原因（**保留失败环节**，不做有损扁平化）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaError {
    /// 固件访问失败。
    Device(Error),
    /// 分区表解析失败。
    Table(TableError),
    /// ISO9660 解析或读取失败。
    Iso(IsoError),
}

/// 从块设备上取出内核映像：判定分区表 → 选卷 → 读内核。
///
/// 没有分区表时（`Ok(None)`）卷就是**整盘**（`start_lba = 0`）—— ISO 直接挂载正是这种情况。
///
/// 分区表判定需要不止第 0 块（GPT 的头在第 1 块、项表在第 2..34 块），故按 34 块读入；
/// 读不满则按实际能读到的部分判定（不假装读到了）。
pub fn load_kernel_from_device<D: BlockDeviceSource>(
    source: &mut D,
    device: DeviceIndex,
    head: &mut [u8],
    out: &mut [u8],
) -> Result<usize, MediaError> {
    let info = source.device_info(device).map_err(MediaError::Device)?;
    let unit = info.block_size as usize;
    if unit == 0 || head.len() < unit {
        return Err(MediaError::Device(Error::BufferTooSmall));
    }
    // 读入覆盖分区表所需的前若干块（GPT 需要 34 块）。
    let want = core::cmp::min(info.block_count, 34) as u32;
    let mut got = 0u32;
    while got < want {
        let at = got as usize * unit;
        if at + unit > head.len() {
            break;
        }
        source
            .read_blocks(device, got as u64, 1, &mut head[at..at + unit])
            .map_err(MediaError::Device)?;
        got += 1;
    }
    if got == 0 {
        return Err(MediaError::Device(Error::BufferTooSmall));
    }
    let image_end = got as usize * unit;
    let mut partitions = [Partition { start_lba: 0, sector_count: 0 }; 8];
    let table = parse_partition_table(
        &head[..core::cmp::min(512, image_end)],
        &head[..image_end],
        unit as u64,
        &mut partitions,
    )
    .map_err(MediaError::Table)?;
    let (start_lba, sector_count) = match table {
        Some((_, count)) if count > 0 => (partitions[0].start_lba, partitions[0].sector_count),
        _ => (0, info.block_count),
    };
    let volume =
        Volume::new(device, start_lba, sector_count, info.block_size).map_err(MediaError::Device)?;
    read_kernel_image(source, &volume, 2048, head, out).map_err(MediaError::Iso)
}

#[cfg(test)]
mod load_kernel_from_device_tests {
    use super::{MediaError, load_kernel_from_device};
    use firmware::block::{BlockDeviceInfo, BlockDeviceSource, DeviceIndex};
    use firmware::error::Error;
    use std::vec::Vec;

    struct FakeDisk {
        bytes: Vec<u8>,
        block_size: u32,
    }

    impl BlockDeviceSource for FakeDisk {
        fn device_count(&self) -> usize {
            1
        }
        fn device_info(&self, _index: DeviceIndex) -> Result<BlockDeviceInfo, Error> {
            Ok(BlockDeviceInfo {
                block_size: self.block_size,
                block_count: (self.bytes.len() as u32 / self.block_size) as u64,
                read_only: true,
            })
        }
        fn read_blocks(
            &mut self,
            _index: DeviceIndex,
            lba: u64,
            count: u32,
            buffer: &mut [u8],
        ) -> Result<(), Error> {
            let unit = self.block_size as usize;
            let at = (lba as usize).checked_mul(unit).ok_or(Error::InvalidArgument)?;
            let len = (count as usize) * unit;
            let src = self.bytes.get(at..at + len).ok_or(Error::InvalidArgument)?;
            buffer
                .get_mut(..len)
                .ok_or(Error::InvalidArgument)?
                .copy_from_slice(src);
            Ok(())
        }
    }

    #[test]
    fn the_real_iso_yields_the_kernel_through_the_whole_media_path() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let Ok(bytes) = std::fs::read(path) else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut disk = FakeDisk { bytes, block_size: 512 };
        // PVD 在偏移 32768，头缓冲要覆盖它（17 个 2048 逻辑块）。
        let mut head = std::vec![0u8; 17 * 2048];
        let mut out = std::vec![0u8; 24_619_400];
        let len = load_kernel_from_device(&mut disk, DeviceIndex(0), &mut head, &mut out)
            .expect("整条介质路径应成功");
        assert_eq!(len, 24_619_400);
        assert_eq!(&out[..4], b"\x7fELF");
        // 与直接按 extent 33 取到的字节交叉核对。
        let at = 33 * 2048;
        assert_eq!(&out[..64], &disk.bytes[at..at + 64]);
    }

    #[test]
    fn a_device_that_is_not_iso_is_reported_as_such() {
        let mut disk = FakeDisk { bytes: std::vec![0u8; 512 * 4096], block_size: 512 };
        let mut head = std::vec![0u8; 17 * 2048];
        let mut out = std::vec![0u8; 4096];
        let result = load_kernel_from_device(&mut disk, DeviceIndex(0), &mut head, &mut out);
        assert!(matches!(result, Err(MediaError::Iso(_))), "非 ISO 必须如实报错");
    }
}