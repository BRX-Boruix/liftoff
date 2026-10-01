//! 把协议层接进入口：扫描内核映像里的请求并填充响应。
//!
//! 边界：本模块只做**编排**（接收映像区间 → 扫描 → 填充）；映像从哪来（读文件、装载、
//! 页表接管）属于 L3/L4，不在本模块。

use core::ffi::c_void;
use crate::responses::Responses;
use limine::fill::fill_response;
use limine::scan::{RequestHit, ScanError, scan_requests};

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
    let count = scan_chunked(image, 4 << 20, hits)?;
    let mut filled = 0;
    for hit in hits.iter().take(count) {
        if let Some(response) = response_for(hit) {
            fill_response(image, hit, response).map_err(|_| ScanError::UnclosedRegion)?;
            filled += 1;
        }
    }
    Ok(ScanReport { hits: count, filled })
}

/// 扫描映像，并把命中的请求的 `response` 指向 `responses` 里对应的响应结构。
///
/// 我们**不提供**响应的请求（容器里没有）会被跳过：既不写入，也不计入 `filled`
/// （故 `filled <= hits` 恒成立 —— **不虚报**填充数）。
pub fn fill_responses(
    image: &mut [u8],
    hits: &mut [RequestHit],
    responses: &mut Responses,
) -> Result<ScanReport, ScanError> {
    let count = scan_requests(image, hits)?;
    let mut filled = 0;
    for index in 0..count {
        let hit = hits[index];
        if let Some(pointer) = responses.pointer_for(&hit.id) {
            fill_response(image, &hit, pointer).map_err(|_| ScanError::UnclosedRegion)?;
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
    use limine::scan::{RequestHit, END_MARKER, START_MARKER};
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
    fn an_image_without_markers_is_scanned_by_id_prefix_instead_of_rejected() {
        // **行为变更（有意）**：真实内核里 START/END 标记被链接器丢掉了，而请求本身都在。
        // 若坚持「无标记即报错」，响应会全部落空（真机上就是如此）。
        // 现在无标记时按请求 ID 前缀扫描 —— 全零映像自然得到 0 命中。
        let mut image = std::vec![0u8; 64];
        let mut hits = [RequestHit::EMPTY; 4];
        assert_eq!(
            prepare_responses(&mut image, &mut hits, |_| None),
            Ok(crate::protocol::ScanReport { hits: 0, filled: 0 })
        );
    }
}

#[cfg(test)]
mod fill_responses_tests {
    use super::{ScanReport, fill_responses};
    use crate::responses::Responses;
    use limine::base::HHDM_REQUEST_ID;
    use limine::memmap::MEMMAP_REQUEST_ID;
    use limine::scan::{RequestHit, END_MARKER, START_MARKER};

    fn push_words(image: &mut std::vec::Vec<u8>, words: &[u64]) {
        for word in words {
            image.extend_from_slice(&word.to_ne_bytes());
        }
    }

    /// 造一个映像：填充 + START + 给定 ID 的请求 + END。
    fn image_with(ids: &[[u64; 4]]) -> std::vec::Vec<u8> {
        let mut image = std::vec![0u8; 64];
        push_words(&mut image, &START_MARKER);
        for id in ids {
            push_words(&mut image, id);
            push_words(&mut image, &[0, 0, 0]);
        }
        push_words(&mut image, &END_MARKER);
        image
    }

    #[test]
    fn a_known_request_gets_the_container_address_written_into_it() {
        let mut image = image_with(&[HHDM_REQUEST_ID]);
        let mut responses = Responses::new();
        responses.set_hhdm_offset(0xffff_8000_0000_0000);
        let expected = responses.pointer_for(&HHDM_REQUEST_ID).expect("有响应");
        let mut hits = [RequestHit::EMPTY; 4];
        let report: ScanReport =
            fill_responses(&mut image, &mut hits, &mut responses).expect("填充成功");
        assert_eq!(report.hits, 1);
        assert_eq!(report.filled, 1);
        // 请求头里 response 在 +40；把那里读出来应与容器给出的地址一致。
        let at = hits[0].offset + 40;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&image[at..at + 8]);
        assert_eq!(usize::from_ne_bytes(buf), expected as usize);
    }

    #[test]
    fn a_request_we_have_no_response_for_is_left_unfilled() {
        // 用一个不在容器里的 ID：命中数 > 填充数。
        let mut image = image_with(&[[0xAAAA_BBBB_CCCC_DDDD, 1, 2, 3]]);
        let mut responses = Responses::new();
        let mut hits = [RequestHit::EMPTY; 4];
        let report = fill_responses(&mut image, &mut hits, &mut responses).expect("填充成功");
        assert_eq!(report.hits, 0, "未知 ID 不算命中（扫描表里没有它）");
        assert_eq!(report.filled, 0);
    }

    #[test]
    fn two_known_requests_are_both_filled() {
        let mut image = image_with(&[HHDM_REQUEST_ID, MEMMAP_REQUEST_ID]);
        let mut responses = Responses::new();
        responses.set_memmap(&[limine::memmap::MemmapEntry {
            base: 0x1000,
            length: 0x2000,
            kind: limine::memmap::USABLE,
        }]);
        let mut hits = [RequestHit::EMPTY; 4];
        let report = fill_responses(&mut image, &mut hits, &mut responses).expect("填充成功");
        assert_eq!(report.hits, 2);
        assert_eq!(report.filled, 2);
    }

    #[test]
    fn an_image_without_markers_is_scanned_by_id_prefix_instead_of_rejected() {
        // 同 `prepare_responses`：无标记改为按 ID 前缀扫描（真实内核丢了标记）。
        let mut image = std::vec![0u8; 64];
        let mut responses = Responses::new();
        let mut hits = [RequestHit::EMPTY; 4];
        assert_eq!(
            fill_responses(&mut image, &mut hits, &mut responses),
            Ok(crate::protocol::ScanReport { hits: 0, filled: 0 })
        );
    }
}

/// 分块扫描大映像（每块 `chunk` 字节，`chunk == 0` 表示不分块）。
///
/// **为什么需要**：固件里对 24.6 MB 一次性扫描**跑不完**（实测 434 秒无输出），
/// 而同一段映像分块扫描能正常跑完（64 KB × 64 段全部通过）。宿主上全量扫描只要 0.57 秒，
/// 所以这是**固件环境的限制**，不是算法问题 —— 但接口必须按它工作。
///
/// 两个正确性要点：
/// - 请求可能**跨块边界** → 每块与下一块**重叠 32 字节**（一个请求 ID 的长度）；
/// - 重叠会让同一请求被扫到两次 → 按 `offset` **去重**。
pub fn scan_chunked(
    image: &[u8],
    chunk: usize,
    hits: &mut [RequestHit],
) -> Result<usize, ScanError> {
    /// 一个请求 ID 的字节长度：重叠必须至少这么大，否则跨块请求会漏。
    const OVERLAP: usize = 32;
    let chunk = if chunk == 0 { image.len().max(1) } else { chunk };
    let mut count = 0usize;
    let mut part = [RequestHit::EMPTY; 64];
    let mut start = 0usize;
    while start < image.len() {
        let end = core::cmp::min(start + chunk, image.len());
        let found = scan_requests(&image[start..end], &mut part)?;
        for hit in &part[..found] {
            let absolute = RequestHit {
                id: hit.id,
                offset: hit.offset + start,
                size: hit.size,
            };
            // 重叠区域会重复命中同一个请求 —— 按偏移去重。
            if hits[..count].iter().any(|existing| existing.offset == absolute.offset) {
                continue;
            }
            if count == hits.len() {
                return Err(ScanError::TooManyRequests);
            }
            hits[count] = absolute;
            count += 1;
        }
        if end == image.len() {
            break;
        }
        // 回退 OVERLAP 字节以覆盖跨块请求；**必须严格推进**，否则死循环。
        let next = end.saturating_sub(OVERLAP);
        start = if next > start { next } else { end };
    }
    Ok(count)
}

#[cfg(test)]
mod scan_chunked_tests {
    use super::scan_chunked;
    use limine::base::HHDM_REQUEST_ID;
    use limine::scan::RequestHit;

    fn put_id(image: &mut [u8], at: usize, id: &[u64; 4]) {
        for (index, word) in id.iter().enumerate() {
            let off = at + index * 8;
            image[off..off + 8].copy_from_slice(&word.to_ne_bytes());
        }
    }

    #[test]
    fn a_request_straddling_a_chunk_boundary_is_found_exactly_once() {
        // 真实固件里对 24.6 MB 一次性扫描跑不完，分块则正常 —— 但分块必须处理两件事：
        // ① 请求可能**跨块边界**；② 为避免漏掉跨界请求而做的重叠会导致**重复计数**。
        let mut image = std::vec![0u8; 8192];
        let at = 2048 - 16; // 恰好横跨 2048 这条边界
        put_id(&mut image, at, &HHDM_REQUEST_ID);
        let mut hits = [RequestHit::EMPTY; 16];
        let count = scan_chunked(&image, 2048, &mut hits).expect("分块扫描应成功");
        assert_eq!(count, 1, "跨块请求必须找到，且**只算一次**");
        assert_eq!(hits[0].offset, at);
    }

    #[test]
    fn several_requests_across_chunks_are_all_found() {
        let mut image = std::vec![0u8; 16384];
        // 请求必须落在 **8 字节边界** 上（协议要求）—— 我第一次把一处放在 100，
        // 那是 4 的倍数而不是 8 的倍数，扫描按 8 步进自然看不到它：是**测试错了**，不是代码错。
        put_id(&mut image, 96, &HHDM_REQUEST_ID);
        put_id(&mut image, 9000, &HHDM_REQUEST_ID);
        put_id(&mut image, 15000, &HHDM_REQUEST_ID);
        let mut hits = [RequestHit::EMPTY; 16];
        let count = scan_chunked(&image, 4096, &mut hits).expect("分块扫描应成功");
        assert_eq!(count, 3, "三处都必须找到，且不重复");
    }
}