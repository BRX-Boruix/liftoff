//! `BlockIo` 协议与到固件抽象的映射。
//!
//! 边界：本模块属**实现侧**（`crates/efi`）——负责把固件协议翻译成 `firmware::block` 的抽象类型。

use crate::types::Status;
use core::ffi::c_void;
use firmware::block::BlockDeviceInfo;
use firmware::error::Error;

/// `EFI_BLOCK_IO_MEDIA`（UEFI 规范）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlockIoMedia {
    /// 介质标识。
    pub media_id: u32,
    /// 是否可移动介质。
    pub removable_media: u8,
    /// 介质是否存在。
    pub media_present: u8,
    /// 是否为逻辑分区。
    pub logical_partition: u8,
    /// 是否只读。
    pub read_only: u8,
    /// 是否启用写缓存。
    pub write_caching: u8,
    /// 逻辑块大小（字节）。
    pub block_size: u32,
    /// 缓冲对齐要求。
    pub io_align: u32,
    /// 最后一个块的块号（**含尾**）。
    pub last_block: u64,
    /// 最低对齐 LBA。
    pub lowest_aligned_lba: u64,
    /// 每物理块逻辑块数。
    pub logical_blocks_per_physical_block: u32,
    /// 最优传输长度粒度。
    pub optimal_transfer_length_granularity: u32,
}

/// `EFI_BLOCK_READ` 的签名。
pub type ReadBlocks = unsafe extern "efiapi" fn(
    this: *mut BlockIo,
    media_id: u32,
    lba: u64,
    buffer_size: usize,
    buffer: *mut c_void,
) -> Status;

/// `EFI_BLOCK_IO_PROTOCOL`（只声明读取路径；写路径不实现）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlockIo {
    /// 协议修订号。
    pub revision: u64,
    /// 介质描述。
    pub media: *mut BlockIoMedia,
    /// 复位（未使用）。
    pub reset: *mut c_void,
    /// 读块。
    pub read_blocks: ReadBlocks,
    /// 写块（未使用）。
    pub write_blocks: *mut c_void,
    /// 刷写（未使用）。
    pub flush_blocks: *mut c_void,
}

/// 把固件介质描述映射为固件抽象的块设备信息。
///
/// 关键语义：UEFI 的 `last_block` 是**含尾**块号，故块数为 `last_block + 1`；
/// 该加法溢出返回 `Error::InvalidArgument`（不静默回绕）。
pub fn media_to_info(media: &BlockIoMedia) -> Result<BlockDeviceInfo, Error> {
    let block_count = media
        .last_block
        .checked_add(1)
        .ok_or(Error::InvalidArgument)?;
    Ok(BlockDeviceInfo {
        block_size: media.block_size,
        block_count,
        read_only: media.read_only != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{BlockIo, BlockIoMedia, media_to_info};
    use core::mem::{offset_of, size_of};
    use firmware::block::BlockDeviceInfo;
    use firmware::error::Error;

    fn media(block_size: u32, last_block: u64, read_only: u8) -> BlockIoMedia {
        BlockIoMedia {
            media_id: 1,
            removable_media: 0,
            media_present: 1,
            logical_partition: 0,
            read_only,
            write_caching: 0,
            block_size,
            io_align: 0,
            last_block,
            lowest_aligned_lba: 0,
            logical_blocks_per_physical_block: 1,
            optimal_transfer_length_granularity: 0,
        }
    }

    #[test]
    fn block_io_media_layout_matches_the_spec() {
        assert_eq!(size_of::<BlockIoMedia>(), 48);
        assert_eq!(offset_of!(BlockIoMedia, media_id), 0);
        assert_eq!(offset_of!(BlockIoMedia, block_size), 12);
        assert_eq!(offset_of!(BlockIoMedia, io_align), 16);
        assert_eq!(offset_of!(BlockIoMedia, last_block), 24);
        assert_eq!(offset_of!(BlockIoMedia, lowest_aligned_lba), 32);
    }

    #[test]
    fn block_io_protocol_layout_matches_the_spec() {
        assert_eq!(size_of::<BlockIo>(), 48);
        assert_eq!(offset_of!(BlockIo, revision), 0);
        assert_eq!(offset_of!(BlockIo, media), 8);
        assert_eq!(offset_of!(BlockIo, read_blocks), 24);
    }

    #[test]
    fn last_block_is_inclusive() {
        assert_eq!(media_to_info(&media(512, 0, 0)).map(|i: BlockDeviceInfo| i.block_count), Ok(1));
        assert_eq!(media_to_info(&media(512, 7, 0)).map(|i: BlockDeviceInfo| i.block_count), Ok(8));
    }

    #[test]
    fn last_block_overflow_is_reported() {
        assert!(matches!(media_to_info(&media(512, u64::MAX, 0)), Err(Error::InvalidArgument)));
    }

    #[test]
    fn read_only_flag_is_mapped() {
        assert_eq!(media_to_info(&media(512, 7, 1)).map(|i: BlockDeviceInfo| i.read_only), Ok(true));
        assert_eq!(media_to_info(&media(512, 7, 0)).map(|i: BlockDeviceInfo| i.read_only), Ok(false));
    }
}
