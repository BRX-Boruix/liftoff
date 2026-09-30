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
    use crate::base::{BASE_REVISION_MAGIC, COMMON_MAGIC, HHDM_REQUEST_ID};
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