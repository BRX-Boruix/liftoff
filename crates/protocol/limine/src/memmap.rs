//! Limine memory map protocol.
//!
//! Verified against brxLimine/limine-protocol/include/limine.h (2026-09-30): the entry's
//! `type` field is 64-bit, and `response.entries` is a pointer to an array of pointers
//! (not a contiguous array).

use crate::base::COMMON_MAGIC;

/// `LIMINE_MEMMAP_REQUEST_ID`.
pub const MEMMAP_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x67cf3d9d378a806f,
    0xe304acdfc50c3c62,
];

/// Usable memory.
pub const USABLE: u64 = 0;
/// Reserved memory.
pub const RESERVED: u64 = 1;
/// ACPI reclaimable memory.
pub const ACPI_RECLAIMABLE: u64 = 2;
/// ACPI NVS memory.
pub const ACPI_NVS: u64 = 3;
/// Bad memory.
pub const BAD_MEMORY: u64 = 4;
/// Memory reclaimable after boot services exit.
pub const BOOTLOADER_RECLAIMABLE: u64 = 5;
/// Memory occupied by the executable and modules.
pub const EXECUTABLE_AND_MODULES: u64 = 6;
/// Framebuffer memory.
pub const FRAMEBUFFER: u64 = 7;
/// Reserved memory that is mapped.
pub const RESERVED_MAPPED: u64 = 8;

/// `struct limine_memmap_entry`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemmapEntry {
    /// Base address.
    pub base: u64,
    /// Length in bytes.
    pub length: u64,
    /// Type (one of the `LIMINE_MEMMAP_*` values).
    pub kind: u64,
}

/// `struct limine_memmap_response`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemmapResponse {
    /// Response revision.
    pub revision: u64,
    /// Number of entries.
    pub entry_count: u64,
    /// Pointer to an array of entry pointers.
    pub entries: *mut *mut MemmapEntry,
}

/// `struct limine_memmap_request`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemmapRequest {
    /// Request identifier.
    pub id: [u64; 4],
    /// Request revision.
    pub revision: u64,
    /// Response pointer (filled by the bootloader).
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
