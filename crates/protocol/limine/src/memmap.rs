//! Limine 内存映射协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实：条目的 type 是 64 位；
//! 响应的 entries 是“指向指针数组的指针”，不是连续数组。

use crate::base::COMMON_MAGIC;

/// `LIMINE_MEMMAP_REQUEST_ID`.
pub const MEMMAP_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x67cf3d9d378a806f,
    0xe304acdfc50c3c62,
];

/// 可用内存。
pub const USABLE: u64 = 0;
/// 保留内存。
pub const RESERVED: u64 = 1;
/// ACPI 可回收内存。
pub const ACPI_RECLAIMABLE: u64 = 2;
/// ACPI NVS 内存。
pub const ACPI_NVS: u64 = 3;
/// 坏内存。
pub const BAD_MEMORY: u64 = 4;
/// 退出引导服务后可回收的内存。
pub const BOOTLOADER_RECLAIMABLE: u64 = 5;
/// 可执行文件与模块占用的内存。
pub const EXECUTABLE_AND_MODULES: u64 = 6;
/// 帧缓冲内存。
pub const FRAMEBUFFER: u64 = 7;
/// 已建立映射的保留内存。
pub const RESERVED_MAPPED: u64 = 8;

/// `struct limine_memmap_entry`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemmapEntry {
    /// 基地址。
    pub base: u64,
    /// 长度（字节）。
    pub length: u64,
    /// 类型（取值之一见 `LIMINE_MEMMAP_*`）。
    pub kind: u64,
}

/// `struct limine_memmap_response`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemmapResponse {
    /// 响应修订。
    pub revision: u64,
    /// 条目数量。
    pub entry_count: u64,
    /// 指向“条目指针数组”的指针。
    pub entries: *mut *mut MemmapEntry,
}

/// `struct limine_memmap_request`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemmapRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut MemmapResponse,
}

#[cfg(test)]
mod tests {
    use super::{
        ACPI_NVS, ACPI_RECLAIMABLE, BAD_MEMORY, BOOTLOADER_RECLAIMABLE, EXECUTABLE_AND_MODULES,
        FRAMEBUFFER, MEMMAP_REQUEST_ID, MemmapEntry, MemmapRequest, MemmapResponse, RESERVED,
        RESERVED_MAPPED, USABLE,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn entry_layout_matches_the_header() {
        assert_eq!(size_of::<MemmapEntry>(), 24);
        assert_eq!(offset_of!(MemmapEntry, base), 0);
        assert_eq!(offset_of!(MemmapEntry, length), 8);
        assert_eq!(offset_of!(MemmapEntry, kind), 16);
    }

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<MemmapResponse>(), 24);
        assert_eq!(offset_of!(MemmapResponse, revision), 0);
        assert_eq!(offset_of!(MemmapResponse, entry_count), 8);
        assert_eq!(offset_of!(MemmapResponse, entries), 16);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<MemmapRequest>(), 48);
        assert_eq!(offset_of!(MemmapRequest, response), 40);
    }

    #[test]
    fn type_values_match_the_header() {
        assert_eq!(USABLE, 0);
        assert_eq!(RESERVED, 1);
        assert_eq!(ACPI_RECLAIMABLE, 2);
        assert_eq!(ACPI_NVS, 3);
        assert_eq!(BAD_MEMORY, 4);
        assert_eq!(BOOTLOADER_RECLAIMABLE, 5);
        assert_eq!(EXECUTABLE_AND_MODULES, 6);
        assert_eq!(FRAMEBUFFER, 7);
        assert_eq!(RESERVED_MAPPED, 8);
    }

    #[test]
    fn request_id_matches_the_header() {
        assert_eq!(MEMMAP_REQUEST_ID[2], 0x67cf3d9d378a806f);
        assert_eq!(MEMMAP_REQUEST_ID[3], 0xe304acdfc50c3c62);
    }
}
