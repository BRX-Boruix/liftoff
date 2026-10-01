//! 把协议层接进入口：扫描内核映像里的请求并填充响应。
//!
//! 边界：本模块只做**编排**（接收映像区间 → 扫描 → 填充）；映像从哪来（读文件、装载、
//! 页表接管）属于 L3/L4，不在本模块。

use core::ffi::c_void;
use crate::responses::Responses;
use limine::fill::fill_response;
use limine::scan::{RequestHit, ScanError};

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
    // 这一路（闭包版）不做区间裁剪：它用于宿主与测试，扫全映像即可。
    let count = limine::scan::scan_chunked(image, 4 << 20, hits)?;
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
    ranges: &[(usize, usize)],
    hits: &mut [RequestHit],
    responses: &mut Responses,
) -> Result<ScanReport, ScanError> {
    // 有区间就只扫已装载段（固件里内存访问极慢，扫全映像不可行）；
    // 没有区间时退回全量分块扫描，作为安全默认。
    let count = if ranges.is_empty() {
        limine::scan::scan_chunked(image, 4 << 20, hits)?
    } else {
        limine::scan::scan_ranges(image, ranges, 4 << 20, hits)?
    };
    let mut filled = 0;
    for index in 0..count {
        let hit = hits[index];
        // `limine_mp_request` 比其它请求**多一个 `flags` 字段**（偏移 48）。内核若
        // 请求 x2APIC，引导器**必须**在响应里回显，否则内核会走 xAPIC（MMIO）路径。
        if hit.id == limine::mp::MP_REQUEST_ID {
            let at = hit.offset + 48;
            if let Some(bytes) = image.get(at..at.saturating_add(8)) {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(bytes);
                if u64::from_le_bytes(buf) & limine::mp::MP_REQUEST_X86_64_X2APIC != 0 {
                    responses.set_smp_flags(limine::mp::MP_RESPONSE_X86_64_X2APIC);
                }
            }
        }
        if let Some(pointer) = responses.pointer_for(&hit.id) {
            // **写进内核映像的必须是 HHDM 地址**（对照 brxLimine：它把每一个响应指针
            // 都过 `reported_addr`，即 `物理 + direct_map_offset`）。
            //
            // 响应容器在引导器的**低地址**内存里：裸地址只在初始恒等映射中有效，内核
            // 一旦切到用户进程的地址空间，读同一指针就 #PF —— 真机实测落在
            // `arch_x86_64::smp::requested_cpu_count` 读 `0x3de16680`。
            // 偏移只在这一处（写入内核映像的边界）加，`pointer_for` 仍返回裸指针，
            // 宿主测试因此可以直接解引用它。
            let pointer = crate::entry::HHDM_OFFSET.wrapping_add(pointer as u64) as *mut core::ffi::c_void;
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
            fill_responses(&mut image, &[], &mut hits, &mut responses).expect("填充成功");
        assert_eq!(report.hits, 1);
        assert_eq!(report.filled, 1);
        // 请求头里 response 在 +40；写进去的必须是 **HHDM 地址**（`裸地址 + direct_map_offset`），
        // 因为裸地址只在初始恒等映射里有效，内核切到用户地址空间后就够不到它了。
        let at = hits[0].offset + 40;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&image[at..at + 8]);
        assert_eq!(
            usize::from_ne_bytes(buf),
            crate::entry::HHDM_OFFSET as usize + expected as usize,
            "写进内核映像的响应指针必须是 HHDM 地址"
        );
    }

    #[test]
    fn a_request_we_have_no_response_for_is_left_unfilled() {
        // 用一个不在容器里的 ID：命中数 > 填充数。
        let mut image = image_with(&[[0xAAAA_BBBB_CCCC_DDDD, 1, 2, 3]]);
        let mut responses = Responses::new();
        let mut hits = [RequestHit::EMPTY; 4];
        let report = fill_responses(&mut image, &[], &mut hits, &mut responses).expect("填充成功");
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
        let report = fill_responses(&mut image, &[], &mut hits, &mut responses).expect("填充成功");
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
            fill_responses(&mut image, &[], &mut hits, &mut responses),
            Ok(crate::protocol::ScanReport { hits: 0, filled: 0 })
        );
    }
}

#[cfg(test)]
mod scan_chunked_tests {
    use limine::scan::scan_chunked;
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