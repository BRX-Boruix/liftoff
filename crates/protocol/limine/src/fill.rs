//! 响应填充：把命中的请求的 `response` 指针指向引导器准备的响应结构。
//!
//! 协议语义：内核在映像里声明请求；引导器把请求的 `response` 字段写成指向**引导器自己**
//! 填好的响应结构。
//!
//! 边界：本函数收 `&mut [u8]` 而非裸指针，于是“请求所在内存必须可写”由**类型**保证，
//! 不需要 `unsafe`。

use crate::scan::RequestHit;

/// `response` 字段在请求头里的偏移（`id[4]` 32 字节 + `revision` 8 字节）。
pub const RESPONSE_OFFSET: usize = 40;

/// 填充失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FillError {
    /// 请求或其 `response` 字段超出给定映像范围。
    OutOfBounds,
}

/// 把 `hit` 的 `response` 指针写成 `response`。
pub fn fill_response(
    image: &mut [u8],
    hit: &RequestHit,
    response: *mut core::ffi::c_void,
) -> Result<(), FillError> {
    let at = hit
        .offset
        .checked_add(RESPONSE_OFFSET)
        .ok_or(FillError::OutOfBounds)?;
    let end = at.checked_add(8).ok_or(FillError::OutOfBounds)?;
    let slot = image.get_mut(at..end).ok_or(FillError::OutOfBounds)?;
    slot.copy_from_slice(&(response as usize).to_ne_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{FillError, RESPONSE_OFFSET, fill_response};
    use crate::base::HHDM_REQUEST_ID;
    use crate::scan::RequestHit;
    

    fn hit_at(offset: usize) -> RequestHit {
        RequestHit { id: HHDM_REQUEST_ID, offset, size: 48 }
    }

    #[test]
    fn the_response_pointer_is_written_at_offset_40() {
        let mut image = std::vec![0u8; 128];
        let hit = hit_at(16);
        let response = 0x1234usize as *mut core::ffi::c_void;
        fill_response(&mut image, &hit, response).expect("填充成功");
        let at = 16 + RESPONSE_OFFSET;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&image[at..at + 8]);
        assert_eq!(usize::from_ne_bytes(buf), 0x1234);
    }

    #[test]
    fn an_out_of_bounds_request_is_rejected() {
        // 40 字节装不下 offset 0 处的 response 槽（40..48）。
        let mut image = std::vec![0u8; 40];
        assert_eq!(
            fill_response(&mut image, &hit_at(0), core::ptr::null_mut()),
            Err(FillError::OutOfBounds)
        );
        assert_eq!(
            fill_response(&mut image, &hit_at(usize::MAX - 8), core::ptr::null_mut()),
            Err(FillError::OutOfBounds),
            "偏移加法溢出也要报错，不能回绕"
        );
    }

    #[test]
    fn filling_does_not_touch_bytes_outside_the_slot() {
        let mut image = std::vec![0xAAu8; 128];
        fill_response(&mut image, &hit_at(16), 1usize as *mut core::ffi::c_void).expect("填充成功");
        let at = 16 + RESPONSE_OFFSET;
        assert_eq!(image[at - 1], 0xAA);
        assert_eq!(image[at + 8], 0xAA);
    }
}
