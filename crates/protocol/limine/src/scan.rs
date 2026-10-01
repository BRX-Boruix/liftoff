//! 请求扫描：在已装载的内核映像里找出 Limine 请求。
//!
//! 规则（对照 brxLimine/limine-protocol/PROTOCOL.md 核实）：
//! - 起止标记必须落在 8 字节对齐边界上；
//! - 只接受**最后一个 START** 与**第一个 END** 之间的请求；
//! - 区域内**按已知请求 ID 逐字比对**（请求不是连续数组，没有固定步长）；
//! - 命中后按**该类型已知的尺寸**前进。

use crate::base::HHDM_REQUEST_ID;
use crate::bootloader_info::BOOTLOADER_INFO_REQUEST_ID;
use crate::entry_point::ENTRY_POINT_REQUEST_ID;
use crate::executable_address::EXECUTABLE_ADDRESS_REQUEST_ID;
use crate::executable_file::EXECUTABLE_FILE_REQUEST_ID;
use crate::firmware_type::FIRMWARE_TYPE_REQUEST_ID;
use crate::framebuffer::FRAMEBUFFER_REQUEST_ID;
use crate::memmap::MEMMAP_REQUEST_ID;
use crate::module::MODULE_REQUEST_ID;
use crate::mp::MP_REQUEST_ID;
use crate::rsdp::RSDP_REQUEST_ID;
use core::mem::size_of;

/// `LIMINE_REQUESTS_START_MARKER`（4×u64）。
pub const START_MARKER: [u64; 4] = [
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
];

/// `LIMINE_REQUESTS_END_MARKER`（2×u64）。
pub const END_MARKER: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

/// 已知请求的（ID, 结构尺寸）。只列本 crate 已实现的结构。
pub const KNOWN_REQUESTS: &[(&[u64; 4], usize)] = &[
    (&HHDM_REQUEST_ID, size_of::<crate::base::HhdmRequest>()),
    (&MEMMAP_REQUEST_ID, size_of::<crate::memmap::MemmapRequest>()),
    (&FRAMEBUFFER_REQUEST_ID, size_of::<crate::framebuffer::FramebufferRequest>()),
    (&RSDP_REQUEST_ID, size_of::<crate::rsdp::RsdpRequest>()),
    (&MP_REQUEST_ID, size_of::<crate::mp::MpRequest>()),
    (&ENTRY_POINT_REQUEST_ID, size_of::<crate::entry_point::EntryPointRequest>()),
    (&EXECUTABLE_FILE_REQUEST_ID, size_of::<crate::executable_file::ExecutableFileRequest>()),
    (&MODULE_REQUEST_ID, size_of::<crate::module::ModuleRequest>()),
    (&BOOTLOADER_INFO_REQUEST_ID, size_of::<crate::bootloader_info::BootloaderInfoRequest>()),
    (&FIRMWARE_TYPE_REQUEST_ID, size_of::<crate::firmware_type::FirmwareTypeRequest>()),
    (&EXECUTABLE_ADDRESS_REQUEST_ID, size_of::<crate::executable_address::ExecutableAddressRequest>()),
];

/// 扫描失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScanError {
    /// 映像里没有请求段起始标记。
    NoStartMarker,
    /// 起始标记之后没有结束标记。
    UnclosedRegion,
    /// 调用方给的命中缓冲太小。
    TooManyRequests,
}

/// 命中的请求（只记录定位信息，填充由调用方按类型处理）。
#[derive(Clone, Copy, Debug)]
pub struct RequestHit {
    /// 请求 ID。
    pub id: [u64; 4],
    /// 在映像里的字节偏移。
    pub offset: usize,
    /// 该类型结构的尺寸。
    pub size: usize,
}

impl RequestHit {
    /// 占位值（便于调用方初始化数组）。
    pub const EMPTY: Self = Self { id: [0; 4], offset: 0, size: 0 };
}

fn read_u64(image: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let bytes = image.get(offset..end)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    Some(u64::from_ne_bytes(buf))
}

fn matches(image: &[u8], offset: usize, id: &[u64; 4]) -> bool {
    (0..4).all(|i| read_u64(image, offset + i * 8) == Some(id[i]))
}

/// 扫描映像里的请求，返回命中数量（写入 `hits`）。
pub fn scan(image: &[u8], hits: &mut [RequestHit]) -> Result<usize, ScanError> {
    // 最后一个 START（8 字节对齐）。
    let mut begin: Option<usize> = None;
    let mut offset = 0;
    while offset + 32 <= image.len() {
        if matches(image, offset, &START_MARKER) {
            begin = Some(offset + 32);
        }
        offset += 8;
    }
    let begin = begin.ok_or(ScanError::NoStartMarker)?;

    // 第一个 END（8 字节对齐）。
    let mut limit: Option<usize> = None;
    let mut offset = begin;
    while offset + 16 <= image.len() {
        if read_u64(image, offset) == Some(END_MARKER[0])
            && read_u64(image, offset + 8) == Some(END_MARKER[1])
        {
            limit = Some(offset);
            break;
        }
        offset += 8;
    }
    let limit = limit.ok_or(ScanError::UnclosedRegion)?;

    // 区域内按已知 ID 比对。
    let mut count = 0;
    let mut offset = begin;
    while offset + 32 <= limit {
        let mut stepped = false;
        for (id, size) in KNOWN_REQUESTS.iter() {
            // 整个请求必须落在区域内，否则不认（不越界读）。
            if offset + *size > limit {
                continue;
            }
            if matches(image, offset, *id) {
                if count == hits.len() {
                    return Err(ScanError::TooManyRequests);
                }
                hits[count] = RequestHit { id: **id, offset, size: *size };
                count += 1;
                offset += *size;
                stepped = true;
                break;
            }
        }
        if !stepped {
            offset += 8;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{KNOWN_REQUESTS, ScanError, scan};
    use crate::base::{BASE_REVISION_MAGIC, HHDM_REQUEST_ID};
    use crate::memmap::MEMMAP_REQUEST_ID;
    use std::vec::Vec;

    /// 请求段起止标记（PROTOCOL.md 核实：START 4×u64，END 2×u64）。
    pub(super) const START: [u64; 4] = [
        0xf6b8f4b39de7d1ae,
        0xfab91a6940fcb9cf,
        0x785c6ed015d3e316,
        0x181e920a7852b9d9,
    ];
    pub(super) const END: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

    fn push_words(image: &mut Vec<u8>, words: &[u64]) {
        for word in words {
            image.extend_from_slice(&word.to_ne_bytes());
        }
    }

    /// 造一个假映像：填充 + START + 一个 hhdm 请求 + 一个 memmap 请求 + END。
    fn image_with_two_requests() -> Vec<u8> {
        let mut image = std::vec![0u8; 64];
        push_words(&mut image, &START);
        let hhdm_offset = image.len();
        push_words(&mut image, &HHDM_REQUEST_ID);
        push_words(&mut image, &[BASE_REVISION_MAGIC[0], 0, 0]);
        let memmap_offset = image.len();
        push_words(&mut image, &MEMMAP_REQUEST_ID);
        push_words(&mut image, &[BASE_REVISION_MAGIC[0], 0, 0]);
        push_words(&mut image, &END);
        assert_eq!(hhdm_offset, 96);
        assert_eq!(memmap_offset, 96 + 32 + 24);
        image
    }

    #[test]
    fn scanning_finds_known_requests_between_the_markers() {
        let image = image_with_two_requests();
        let mut hits = [super::RequestHit::EMPTY; 4];
        let count = scan(&image, &mut hits).expect("扫描成功");
        assert_eq!(count, 2);
        assert_eq!(hits[0].id, HHDM_REQUEST_ID);
        assert_eq!(hits[0].offset, 96);
        assert_eq!(hits[1].id, MEMMAP_REQUEST_ID);
        assert_eq!(KNOWN_REQUESTS.len() >= 2, true);
    }

    #[test]
    fn an_unclosed_region_is_rejected() {
        let mut image = std::vec![0u8; 32];
        push_words(&mut image, &START);
        push_words(&mut image, &HHDM_REQUEST_ID);
        let mut hits = [super::RequestHit::EMPTY; 4];
        assert_eq!(scan(&image, &mut hits), Err(ScanError::UnclosedRegion));
    }

    #[test]
    fn a_missing_start_marker_is_rejected() {
        let image = std::vec![0u8; 64];
        let mut hits = [super::RequestHit::EMPTY; 4];
        assert_eq!(scan(&image, &mut hits), Err(ScanError::NoStartMarker));
    }
}

/// 扫描请求：**先按起止标记**；映像里没有标记时，退回**按请求 ID 前缀**定位。
///
/// **偏离说明**：协议规定用起止标记圈定区域。这里回退的依据是协议自身的另一条不变量 ——
/// **每个请求 ID 都以相同的两个魔数开头**。实测真实内核里 START/END 标记出现 0 次
/// （链接器把它们丢了）而请求本身都在，导致按标记扫描**一个都找不到**、响应全部落空。
/// 标记存在时**优先用标记**，以保证「标记外的不算」这一语义不被削弱。
pub fn scan_requests(image: &[u8], hits: &mut [RequestHit]) -> Result<usize, ScanError> {
    match scan(image, hits) {
        Ok(count) => Ok(count),
        Err(ScanError::NoStartMarker) | Err(ScanError::UnclosedRegion) => {
            scan_by_prefix(image, hits)
        }
        Err(other) => Err(other),
    }
}

/// 按请求 ID 前缀扫描（无标记时的回退）。
fn scan_by_prefix(image: &[u8], hits: &mut [RequestHit]) -> Result<usize, ScanError> {
    let mut count = 0usize;
    let mut at = 0usize;
    while at + 32 <= image.len() {
        // 先比**第一个词**（协议不变量：每个请求 ID 都以同一个魔数开头）。
        // 不做这一步就要对每个位置都比 4 个词 × 11 个 ID —— 在 24.6 MB 的映像上
        // 那是上亿次读取，真机上表现为长时间无输出（实测卡在 `before scan`）。
        if read_u64(image, at) != Some(crate::base::COMMON_MAGIC[0]) {
            at += 8;
            continue;
        }
        let mut found = None;
        for (id, size) in KNOWN_REQUESTS {
            if matches(image, at, id) {
                found = Some((id, size));
                break;
            }
        }
        if let Some((id, size)) = found {
            if count == hits.len() {
                return Err(ScanError::TooManyRequests);
            }
            hits[count] = RequestHit { id: **id, offset: at, size: *size };
            count += 1;
            // 命中后按该类型已知尺寸前进（与按标记扫描同一套规则）。
            // **必须保证推进**：尺寸为 0（或小于一个 ID）时若原地不动就是死循环 ——
            // 真机上正是卡在这里（`before scan` 之后再也没有输出）。
            let step = if *size >= 8 { *size } else { 8 };
            at += step;
        } else {
            // **只有未命中**才按 8 字节对齐步进（请求必须落在 8 字节边界上）。
            // 之前这一句在 `if` 之外，命中后会**双重推进**，把紧随其后的请求跳过。
            at += 8;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod scan_requests_tests {
    use super::{HHDM_REQUEST_ID, RequestHit, scan_requests};
    use crate::base::COMMON_MAGIC;

    /// 把 4 个 ID 词写进映像（按 8 字节对齐）。
    fn put_id(image: &mut [u8], at: usize, id: &[u64; 4]) {
        for (index, word) in id.iter().enumerate() {
            let off = at + index * 8;
            image[off..off + 8].copy_from_slice(&word.to_ne_bytes());
        }
    }

    #[test]
    fn a_kernel_without_markers_is_still_scanned_by_id_prefix() {
        // 真实内核里 START/END 标记被链接器丢掉了（实测 0 次），但请求本身在。
        // 每个请求 ID 都以相同的两个魔数开头 —— 这是协议自身的不变量，可作为回退依据。
        let mut image = std::vec![0u8; 256];
        let at = 64usize;
        put_id(&mut image, at, &HHDM_REQUEST_ID);
        let mut hits = [RequestHit::EMPTY; 8];
        let count = scan_requests(&image, &mut hits).expect("无标记也必须能扫");
        assert_eq!(count, 1);
        assert_eq!(hits[0].id, HHDM_REQUEST_ID);
        assert_eq!(hits[0].offset, at);
    }

    #[test]
    fn a_bare_common_magic_is_not_a_request() {
        // 只有前两个魔数、后两个词不是已知 ID —— **不得**产生命中（否则会误填随机数据）。
        let mut image = std::vec![0u8; 256];
        put_id(&mut image, 64, &[COMMON_MAGIC[0], COMMON_MAGIC[1], 1, 2]);
        let mut hits = [RequestHit::EMPTY; 8];
        let count = scan_requests(&image, &mut hits).expect("扫描应成功");
        assert_eq!(count, 0, "只有魔数前缀不算请求");
    }

    #[test]
    fn markers_still_take_precedence_when_present() {
        // 有标记时走原路径：标记外的同名 ID **不应**被算进来。
        let mut image = std::vec![0u8; 512];
        // 标记外放一个 HHDM 请求（偏移 16），标记内不放。
        put_id(&mut image, 16, &HHDM_REQUEST_ID);
        let mut at = 128usize;
        for word in super::START_MARKER {
            image[at..at + 8].copy_from_slice(&word.to_ne_bytes());
            at += 8;
        }
        for word in super::END_MARKER {
            image[at..at + 8].copy_from_slice(&word.to_ne_bytes());
            at += 8;
        }
        let mut hits = [RequestHit::EMPTY; 8];
        let count = scan_requests(&image, &mut hits).expect("扫描应成功");
        assert_eq!(count, 0, "有标记时以标记为准，标记外的不算");
    }
}

#[cfg(test)]
mod real_kernel_scan_tests {
    use super::{HHDM_REQUEST_ID, MEMMAP_REQUEST_ID, RequestHit, scan_requests};

    /// 从真实 ISO 里取出内核映像（extent 33、24,619,400 字节）。
    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../boruix.iso");
        let bytes = std::fs::read(iso).ok()?;
        bytes.get(33 * 2048..33 * 2048 + 24_619_400).map(|s| s.to_vec())
    }

    #[test]
    fn the_real_kernel_requests_are_found_without_markers() {
        let Some(image) = real_kernel() else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut hits = [RequestHit::EMPTY; 64];
        let count = scan_requests(&image, &mut hits).expect("扫描应成功");
        std::eprintln!("真实内核里找到 {} 个请求", count);
        assert!(count >= 5, "真实内核声明了多个请求（实测 COMMON_MAGIC 出现 7 次），实得 {count}");
        let has = |id: &[u64; 4]| hits[..count].iter().any(|hit| &hit.id == id);
        assert!(has(&HHDM_REQUEST_ID), "必须找到 HHDM 请求");
        assert!(has(&MEMMAP_REQUEST_ID), "必须找到内存映射请求");
    }
}
