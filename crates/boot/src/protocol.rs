//! 把协议层接进入口：扫描内核映像里的请求并填充响应。
//!
//! 边界：本模块只做**编排**（接收映像区间 → 扫描 → 填充）；映像从哪来（读文件、装载、
//! 页表接管）属于 L3/L4，不在本模块。

use core::ffi::c_void;
use limine::fill::fill_response;
use limine::scan::{RequestHit, ScanError, scan};

/// 扫描结果摘要（便于测试断言与诊断）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScanReport {
    /// 命中的请求数量。
    pub hits: usize,
    /// 成功写入 response 指针的数量。
    pub filled: usize,
}

/// 在映像里扫描请求，并按 `response_for` 给出的指针填充每个命中的 `response` 字段。
///
/// `response_for` 返回 `None` 表示该请求本轮不填充（跳过，不计入 `filled`）。
pub fn prepare_responses<F>(
    image: &mut [u8],
    hits: &mut [RequestHit],
    mut response_for: F,
) -> Result<ScanReport, ScanError>
where
    F: FnMut(&RequestHit) -> Option<*mut c_void>,
{
    let count = scan(image, hits)?;
    let mut filled = 0;
    for hit in hits.iter().take(count) {
        if let Some(response) = response_for(hit) {
            fill_response(image, hit, response).map_err(|_| ScanError::UnclosedRegion)?;
            filled += 1;
        }
    }
    Ok(ScanReport { hits: count, filled })
}

#[cfg(test)]
mod tests {
    use super::prepare_responses;
    use core::ffi::c_void;
    use limine::base::HHDM_REQUEST_ID;
    use limine::scan::{RequestHit, ScanError, END_MARKER, START_MARKER};
    use std::vec::Vec;

    fn push_words(image: &mut Vec<u8>, words: &[u64]) {
        for word in words {
            image.extend_from_slice(&word.to_ne_bytes());
        }
    }

    fn image_with_hhdm() -> Vec<u8> {
        let mut image = std::vec![0u8; 64];
        push_words(&mut image, &START_MARKER);
        push_words(&mut image, &HHDM_REQUEST_ID);
        push_words(&mut image, &[0, 0, 0]);
        push_words(&mut image, &END_MARKER);
        image
    }

    #[test]
    fn scanning_and_filling_work_together() {
        let mut image = image_with_hhdm();
        let mut hits = [RequestHit::EMPTY; 4];
        let fake_response = 0x5678usize as *mut c_void;
        let report = prepare_responses(&mut image, &mut hits, |_| Some(fake_response)).expect("编排成功");
        assert_eq!(report.hits, 1);
        assert_eq!(report.filled, 1);
        // response 指针写在 hits[0].offset + 40 处。
        let at = hits[0].offset + 40;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&image[at..at + 8]);
        assert_eq!(usize::from_ne_bytes(buf), 0x5678);
    }

    #[test]
    fn skipping_a_request_leaves_it_unfilled() {
        let mut image = image_with_hhdm();
        let mut hits = [RequestHit::EMPTY; 4];
        let report = prepare_responses(&mut image, &mut hits, |_| None).expect("编排成功");
        assert_eq!(report.hits, 1);
        assert_eq!(report.filled, 0);
    }

    #[test]
    fn an_image_without_markers_is_rejected() {
        let mut image = std::vec![0u8; 64];
        let mut hits = [RequestHit::EMPTY; 4];
        assert_eq!(
            prepare_responses(&mut image, &mut hits, |_| None),
            Err(ScanError::NoStartMarker)
        );
    }
}
