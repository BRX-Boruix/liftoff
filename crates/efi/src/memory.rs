//! UEFI 内存描述符与到固件抽象的映射。
//!
//! 关键点：**UEFI 的内存类型枚举与 Limine 不是同一套**，必须显式映射；
//! 未列出的类型一律 `Error::Unsupported`（不猜）。

use arch::addr::PhysAddr;
use firmware::error::Error;
use firmware::memory::{MemoryEntry, MemoryKind};

/// 页大小（UEFI 固定 4 KiB）。
pub const PAGE_SIZE: u64 = 4096;

/// UEFI 内存类型（`EFI_MEMORY_TYPE`，UEFI 规范）。
pub const RESERVED: u32 = 0;
/// 引导器代码。
pub const LOADER_CODE: u32 = 1;
/// 引导器数据。
pub const LOADER_DATA: u32 = 2;
/// 引导服务代码。
pub const BOOT_SERVICES_CODE: u32 = 3;
/// 引导服务数据。
pub const BOOT_SERVICES_DATA: u32 = 4;
/// 运行期服务代码。
pub const RUNTIME_SERVICES_CODE: u32 = 5;
/// 运行期服务数据。
pub const RUNTIME_SERVICES_DATA: u32 = 6;
/// 常规可用内存。
pub const CONVENTIONAL: u32 = 7;
/// 不可用内存。
pub const UNUSABLE: u32 = 8;
/// ACPI 可回收。
pub const ACPI_RECLAIM: u32 = 9;
/// ACPI NVS。
pub const ACPI_NVS: u32 = 10;
/// 内存映射 IO。
pub const MMIO: u32 = 11;
/// 内存映射 IO 端口空间。
pub const MMIO_PORT: u32 = 12;
/// PalCode。
pub const PAL_CODE: u32 = 13;
/// 持久内存。
pub const PERSISTENT: u32 = 14;

/// `EFI_MEMORY_DESCRIPTOR`（UEFI 规范）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemoryDescriptor {
    /// 内存类型。
    pub memory_type: u32,
    /// 填充（保持 8 字节对齐）。
    pub pad: u32,
    /// 物理起始地址。
    pub physical_start: u64,
    /// 虚拟起始地址（引导阶段未使用）。
    pub virtual_start: u64,
    /// 页数。
    pub number_of_pages: u64,
    /// 属性位。
    pub attribute: u64,
}

/// UEFI 内存类型 → 抽象类型。
///
/// 映射理由（逐条）：
/// - `Conventional` → `Usable`：常规可用内存。
/// - `ACPIReclaim`/`ACPIMemoryNVS` → 对应 ACPI 类型。
/// - `Loader*`/`BootServices*` → `BootloaderReclaimable`：退出引导服务后即可回收。
/// - `Reserved`/`RuntimeServices*`/`Unusable`/`MMIO`/`PalCode`/`Persistent` → `Reserved`：
///   内核都不得当作普通可用内存使用（`Unusable` 与 MMIO 尤其如此）。
pub const fn classify(memory_type: u32) -> Result<MemoryKind, Error> {
    match memory_type {
        CONVENTIONAL => Ok(MemoryKind::Usable),
        ACPI_RECLAIM => Ok(MemoryKind::AcpiReclaimable),
        ACPI_NVS => Ok(MemoryKind::AcpiNvs),
        LOADER_CODE | LOADER_DATA | BOOT_SERVICES_CODE | BOOT_SERVICES_DATA => {
            Ok(MemoryKind::BootloaderReclaimable)
        }
        RESERVED | RUNTIME_SERVICES_CODE | RUNTIME_SERVICES_DATA | UNUSABLE | MMIO | MMIO_PORT
        | PAL_CODE | PERSISTENT => Ok(MemoryKind::Reserved),
        _ => Err(Error::Unsupported),
    }
}

/// 把一条 UEFI 描述符转换为抽象条目；页数换算溢出返回 `Error::InvalidArgument`。
pub const fn descriptor_to_entry(descriptor: &MemoryDescriptor) -> Result<MemoryEntry, Error> {
    let length = match descriptor.number_of_pages.checked_mul(PAGE_SIZE) {
        Some(bytes) => bytes,
        None => return Err(Error::InvalidArgument),
    };
    match classify(descriptor.memory_type) {
        Ok(kind) => Ok(MemoryEntry {
            base: PhysAddr::new(descriptor.physical_start),
            length,
            kind,
        }),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryDescriptor, classify, descriptor_to_entry};
    use core::mem::{offset_of, size_of};
    use firmware::error::Error;
    use firmware::memory::MemoryKind;

    fn descriptor(memory_type: u32, pages: u64) -> MemoryDescriptor {
        MemoryDescriptor {
            memory_type,
            pad: 0,
            physical_start: 0x1000,
            virtual_start: 0,
            number_of_pages: pages,
            attribute: 0,
        }
    }

    #[test]
    fn descriptor_layout_matches_the_spec() {
        assert_eq!(size_of::<MemoryDescriptor>(), 40);
        assert_eq!(offset_of!(MemoryDescriptor, memory_type), 0);
        assert_eq!(offset_of!(MemoryDescriptor, physical_start), 8);
        assert_eq!(offset_of!(MemoryDescriptor, virtual_start), 16);
        assert_eq!(offset_of!(MemoryDescriptor, number_of_pages), 24);
        assert_eq!(offset_of!(MemoryDescriptor, attribute), 32);
    }

    #[test]
    fn uefi_types_map_to_the_abstract_kinds() {
        assert_eq!(classify(7), Ok(MemoryKind::Usable));
        assert_eq!(classify(0), Ok(MemoryKind::Reserved));
        assert_eq!(classify(9), Ok(MemoryKind::AcpiReclaimable));
        assert_eq!(classify(10), Ok(MemoryKind::AcpiNvs));
        assert_eq!(classify(3), Ok(MemoryKind::BootloaderReclaimable));
    }

    #[test]
    fn unknown_uefi_types_are_rejected() {
        assert_eq!(classify(15), Err(Error::Unsupported));
        assert_eq!(classify(u32::MAX), Err(Error::Unsupported));
    }

    #[test]
    fn pages_convert_to_bytes() {
        let entry = descriptor_to_entry(&descriptor(7, 2)).expect("可用内存");
        assert_eq!(entry.base.as_u64(), 0x1000);
        assert_eq!(entry.length, 8192);
        assert_eq!(entry.kind, MemoryKind::Usable);
    }

    #[test]
    fn page_count_overflow_is_reported() {
        assert!(matches!(descriptor_to_entry(&descriptor(7, u64::MAX)), Err(Error::InvalidArgument)));
    }
}
