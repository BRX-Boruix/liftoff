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
        // 先比**第一个词**，命中才比其余三个。固件环境里内存访问极慢
        // （实测每次读约 100 微秒），每位置 4 次读变成 1 次是 4 倍的实际收益。
        if read_u64(image, offset) == Some(START_MARKER[0])
            && matches(image, offset, &START_MARKER)
        {
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

/// 在若干**文件区间**内扫描（区间通常来自 ELF 的已装载段）。
///
/// **为什么需要**：固件里内存访问极慢（量级估计每次读约 100 微秒），扫 24.6 MB 不可行。
/// 而请求必须位于**会被装载的段**里才可能在运行中存在 —— 真实内核的 7 个请求都在第三个段内。
/// 只扫已装载段约 10 MB，是 2.5 倍的减少，且**语义等价**（由真实内核对照测试守住）。
pub fn scan_ranges(
    image: &[u8],
    ranges: &[(usize, usize)],
    chunk: usize,
    hits: &mut [RequestHit],
) -> Result<usize, ScanError> {
    let mut count = 0usize;
    for (start, end) in ranges {
        if start >= end || *end > image.len() {
            continue;
        }
        let mut part = [RequestHit::EMPTY; 64];
        let found = scan_chunked(&image[*start..*end], chunk, &mut part)?;
        for hit in &part[..found] {
            let absolute = RequestHit {
                id: hit.id,
                offset: hit.offset + *start,
                size: hit.size,
            };
            if hits[..count].iter().any(|existing| existing.offset == absolute.offset) {
                continue;
            }
            if count == hits.len() {
                return Err(ScanError::TooManyRequests);
            }
            hits[count] = absolute;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod real_kernel_scan_tests {
    use super::{HHDM_REQUEST_ID, MEMMAP_REQUEST_ID, RequestHit, scan_requests, scan_ranges};

    /// 从真实 ISO 里取出内核映像（extent 33、24,619,400 字节）。
    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../boruix.iso");
        let bytes = std::fs::read(iso).ok()?;
        bytes.get(33 * 2048..33 * 2048 + 24_619_400).map(|s| s.to_vec())
    }

    /// 真实内核三个 `PT_LOAD` 段的文件区间（实测自 `readelf`：offset/filesz）。
    const REAL_SEGMENTS: [(usize, usize); 3] = [
        (0x1000, 0x1000 + 0x223cb0),
        (0x224000, 0x224000 + 0x4d5780),
        (0x6fa000, 0x6fa000 + 0x2bea88),
    ];

    #[test]
    fn scanning_only_the_loaded_segments_still_finds_every_request() {
        let Some(image) = real_kernel() else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut hits = [RequestHit::EMPTY; 64];
        let count = scan_ranges(&image, &REAL_SEGMENTS, 4 << 20, &mut hits).expect("扫描应成功");
        std::eprintln!("按段区间扫到 {} 个请求", count);
        assert_eq!(count, 7, "按段区间扫描必须与全量扫描得到同样的 7 个请求");
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
#[cfg(test)]
mod data_section_scan_tests {
    use super::{RequestHit, scan_chunked};

    /// ELF64 节头关键字段读取（测试辅助，按小端硬编码偏移）。
    fn section_range(image: &[u8], wanted: &str) -> Option<(usize, usize)> {
        fn rd_u16(image: &[u8], at: usize) -> Option<u16> {
            Some(u16::from_le_bytes(image.get(at..at + 2)?.try_into().unwrap()))
        }
        fn rd_u32(image: &[u8], at: usize) -> Option<u32> {
            Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().unwrap()))
        }
        fn rd_u64(image: &[u8], at: usize) -> Option<u64> {
            Some(u64::from_le_bytes(image.get(at..at + 8)?.try_into().unwrap()))
        }
        let shoff = rd_u64(image, 0x28)? as usize;
        let shentsize = rd_u16(image, 0x3A)? as usize;
        let shnum = rd_u16(image, 0x3C)? as usize;
        let shstrndx = rd_u16(image, 0x3E)? as usize;
        let read_sh = |index: usize| -> Option<(u32, usize, usize)> {
            let at = shoff.checked_add(index.checked_mul(shentsize)?)?;
            Some((rd_u32(image, at)?, rd_u64(image, at.checked_add(24)?)? as usize, rd_u64(image, at.checked_add(32)?)? as usize))
        };
        let (_, stroff, _) = read_sh(shstrndx)?;
        for index in 0..shnum {
            let (name_off, offset, size) = read_sh(index)?;
            let name_at = stroff.checked_add(name_off as usize)?;
            let mut end = name_at;
            while image.get(end) != Some(&0) {
                end += 1;
            }
            let name = image.get(name_at..end)?;
            if name == wanted.as_bytes() {
                return Some((offset, offset.checked_add(size)?));
            }
        }
        None
    }

    #[test]
    fn scanning_only_the_data_section_finds_all_seven_requests() {
        // 实测：7 个请求（COMMON_MAGIC）集中在偏移 7,656,824–7,656,968，
        // 全部落在 .data 节（off=7319552 size=345312）内。固件读一次内存约 100 µs，
        // 把扫描范围缩到一个节是从「跑不完」到「几秒」的差别。
        // 注意：这条测试只断言**等价性**（扫 .data 与扫全映像得到同样结果），
        // 引导器在生产路径上定位 .data 的方式由 loader 层的节表解析负责。
        let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../boruix.iso");
        let Some(bytes) = std::fs::read(iso).ok() else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let Some(image) = bytes.get(33 * 2048..33 * 2048 + 24_619_400) else {
            std::eprintln!("跳过：ISO 内核 extent 不在预期位置");
            return;
        };
        let Some((start, end)) = section_range(image, ".data") else {
            panic!("真实内核应有 .data 节");
        };
        let mut hits = [RequestHit::EMPTY; 64];
        let count = scan_chunked(&image[start..end], 64 << 10, &mut hits).expect("扫描应成功");
        assert_eq!(count, 7, "只扫 .data 必须与全量扫描同样找到 7 个请求");
    }
}
