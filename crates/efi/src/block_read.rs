//! 读块路径的字节数换算。
//!
//! 边界：只做**算术**；实际的 `BlockIo.ReadBlocks` 调用由 `BlockDeviceSource` 实现负责。

/// 固件读块调用需要的字节数。
///
/// 数值论证（S19）：`count` 与 `block_size` 均为 `u32`，乘积 `<= (2^32-1)^2 < 2^64`；
/// 本目标 `usize` 为 64 位，故转换无损，**不需要 `Option`**。
pub const fn read_byte_len(count: u32, block_size: u32) -> usize {
    (count as u64 * block_size as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::read_byte_len;

    #[test]
    fn byte_len_is_count_times_block_size() {
        assert_eq!(read_byte_len(1, 512), 512);
        assert_eq!(read_byte_len(8, 512), 4096);
        assert_eq!(read_byte_len(0, 512), 0);
    }

    #[test]
    fn the_maximum_product_still_fits_in_usize() {
        // 数值论证：count 与 block_size 均为 u32，乘积 <= (2^32-1)^2 < 2^64。
        let max = u32::MAX as u64 * u32::MAX as u64;
        assert_eq!(read_byte_len(u32::MAX, u32::MAX) as u64, max);
    }
}
