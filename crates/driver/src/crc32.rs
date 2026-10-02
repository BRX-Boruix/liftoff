//! CRC32（IEEE 802.3，反射多项式 `0xEDB88320`）。
//!
//! 用途：校验 GPT 的头与分区项数组 —— **读错分区 = 读错数据**，故不信任未校验的扇区。
//! 实现：**逐位**算法（无需查表，代码小且无静态表）；`const fn` 便于编译期使用。

/// 计算 CRC32。
pub const fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    let mut index = 0;
    while index < bytes.len() {
        crc ^= bytes[index] as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        index += 1;
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn the_standard_check_value_matches() {
        // 规范给出的校验值：CRC32(b"123456789") == 0xCBF43926。
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn an_empty_input_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn different_inputs_give_different_values() {
        assert_ne!(crc32(b"abc"), crc32(b"abd"));
        assert_ne!(crc32(b"a"), crc32(b"aa"));
    }

    #[test]
    fn it_is_deterministic() {
        let bytes = [0u8, 1, 2, 3, 255, 128];
        assert_eq!(crc32(&bytes), crc32(&bytes));
    }
}