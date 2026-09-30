//! 块设备：固件提供的块设备枚举与读取。
//!
//! 边界：只定义“枚举 + 读块”的语义，不出现任何固件专有句柄（设备用序号标识）。

use crate::error::Error;

/// 块设备标识：固件设备的抽象序号（不泄漏固件指针）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeviceIndex(pub u32);

/// 块设备信息。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockDeviceInfo {
    /// 逻辑块大小（字节）。
    pub block_size: u32,
    /// 逻辑块数。
    pub block_count: u64,
    /// 是否只读。
    pub read_only: bool,
}

impl BlockDeviceInfo {
    /// 设备容量（字节）；乘法溢出返回 `None`。
    pub const fn byte_len(self) -> Option<u64> {
        (self.block_size as u64).checked_mul(self.block_count)
    }

    /// 块大小是否成立：非零且为 2 的幂。
    pub const fn is_valid(self) -> bool {
        self.block_size != 0 && self.block_size.is_power_of_two()
    }
}

/// 读块请求的参数校验；各实现共用（单点定义）。
///
/// 约定：`count == 0` 视为调用方错误（`InvalidArgument`），不静默成功（宁可报错）。
pub fn validate_read(info: BlockDeviceInfo, lba: u64, count: u32, buffer_len: usize) -> Result<(), Error> {
    if count == 0 {
        return Err(Error::InvalidArgument);
    }
    let end = lba.checked_add(count as u64).ok_or(Error::InvalidArgument)?;
    if end > info.block_count {
        return Err(Error::InvalidArgument);
    }
    let need = (count as u64)
        .checked_mul(info.block_size as u64)
        .ok_or(Error::InvalidArgument)?;
    if (buffer_len as u64) < need {
        return Err(Error::BufferTooSmall);
    }
    Ok(())
}

/// 块设备枚举与读取：固件层能力 trait 之一。
pub trait BlockDeviceSource {
    /// 设备数量。
    fn device_count(&self) -> usize;

    /// 取设备信息；索引越界返回 `Error::NotFound`。
    fn device_info(&self, index: DeviceIndex) -> Result<BlockDeviceInfo, Error>;

    /// 读逻辑块到调用方缓冲；参数与容量按 [`validate_read`] 校验。
    fn read_blocks(
        &mut self,
        index: DeviceIndex,
        lba: u64,
        count: u32,
        buffer: &mut [u8],
    ) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::{BlockDeviceInfo, DeviceIndex, validate_read};
    use crate::error::Error;

    fn info(block_size: u32, block_count: u64) -> BlockDeviceInfo {
        BlockDeviceInfo { block_size, block_count, read_only: false }
    }

    #[test]
    fn byte_len_reports_overflow() {
        assert_eq!(info(512, 2).byte_len(), Some(1024));
        assert_eq!(info(4096, u64::MAX).byte_len(), None);
    }

    #[test]
    fn block_size_must_be_a_nonzero_power_of_two() {
        assert!(info(512, 1).is_valid());
        assert!(!info(0, 1).is_valid());
        assert!(!info(3, 1).is_valid());
    }

    #[test]
    fn validate_read_accepts_a_well_formed_request() {
        assert_eq!(validate_read(info(512, 8), 0, 8, 4096), Ok(()));
        assert_eq!(validate_read(info(512, 8), 7, 1, 512), Ok(()));
    }

    #[test]
    fn validate_read_reports_out_of_range_and_overflow() {
        assert_eq!(validate_read(info(512, 8), 8, 1, 512), Err(Error::InvalidArgument));
        assert_eq!(validate_read(info(512, 8), u64::MAX, 2, 1024), Err(Error::InvalidArgument));
    }

    #[test]
    fn validate_read_reports_buffer_too_small_and_zero_count() {
        assert_eq!(validate_read(info(512, 8), 0, 2, 1023), Err(Error::BufferTooSmall));
        assert_eq!(validate_read(info(512, 8), 0, 0, 512), Err(Error::InvalidArgument));
    }

    #[test]
    fn device_index_is_a_value_type() {
        assert_eq!(DeviceIndex(0), DeviceIndex(0));
        assert_ne!(DeviceIndex(0), DeviceIndex(1));
    }
}
