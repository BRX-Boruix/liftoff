//! `SystemTable` → 引导服务表：入口的第一个边界。
//!
//! 边界：只做**指针取值与空校验**，不解释引导服务表的内容（那是 `boot_services_table` 的事）。
//! 用裸指针而非引用：固件给的生命周期无从在类型上表达，故不伪造生命周期。

use crate::boot_services_table::BootServicesTable;
use crate::types::SystemTable;
use firmware::error::Error;

/// 从系统表取出引导服务表指针。
///
/// - `system_table` 为空 → `Error::InvalidArgument`（未按契约传参）；
/// - 系统表里的 `boot_services` 为空 → `Error::Io`（固件违约）。
pub fn boot_services_of(system_table: *mut SystemTable) -> Result<*mut BootServicesTable, Error> {
    if system_table.is_null() {
        return Err(Error::InvalidArgument);
    }
    // SAFETY: 指针非空，且由固件保证 `efi_main` 收到的是有效系统表。
    let boot_services = unsafe { (*system_table).boot_services };
    if boot_services.is_null() {
        return Err(Error::Io);
    }
    Ok(boot_services.cast::<BootServicesTable>())
}

#[cfg(test)]
mod tests {
    use super::boot_services_of;
    use crate::types::SystemTable;
    use firmware::error::Error;

    #[test]
    fn a_null_system_table_is_rejected() {
        assert_eq!(boot_services_of(core::ptr::null_mut()), Err(Error::InvalidArgument));
    }

    #[test]
    fn a_null_boot_services_pointer_is_a_firmware_violation() {
        // SAFETY: `SystemTable` 的零值是合法表示（全空指针）。
        let mut table = unsafe { core::mem::zeroed::<SystemTable>() };
        assert_eq!(boot_services_of(&mut table), Err(Error::Io));
    }

    #[test]
    fn a_valid_system_table_yields_the_boot_services_pointer() {
        // SAFETY: 同上；随后只写入一个非空指针。
        let mut table = unsafe { core::mem::zeroed::<SystemTable>() };
        table.boot_services = 0x1234usize as *mut core::ffi::c_void;
        let got = boot_services_of(&mut table).expect("引导服务表可取");
        assert_eq!(got as usize, 0x1234);
    }
}

/// ACPI 2.0 的配置表 GUID（`8868e871-e4f1-11d3-bc22-0080c73c8881`）。
use crate::guid::Guid;
use core::ffi::c_void;

const ACPI_TABLE_GUID: Guid = Guid {
    data1: 0x8868_e871,
    data2: 0xe4f1,
    data3: 0x11d3,
    data4: [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
};

/// 一条 UEFI 配置表项（`EFI_CONFIGURATION_TABLE`）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConfigurationTable {
    /// 供应商标识。
    pub vendor_guid: Guid,
    /// 供应商标（对 ACPI 而言就是 RSDP）。
    pub vendor_table: *mut c_void,
}

/// 从系统表里找 **ACPI 2.0 的 RSDP**；没有就返回 `None` —— **不编造**。
///
/// 内核早期（定时器、中断控制器）很可能需要它；给一个假指针比不给更糟。
pub fn acpi_rsdp(table: &SystemTable) -> Option<*mut c_void> {
    if table.configuration_table.is_null() || table.number_of_table_entries == 0 {
        return None;
    }
    let entries = table.configuration_table as *const ConfigurationTable;
    for index in 0..table.number_of_table_entries {
        // SAFETY: 固件保证配置表有 `number_of_table_entries` 项且指针有效；
        // 用 `read_unaligned` 避免对表项的对齐做假设。
        let entry = unsafe { core::ptr::read_unaligned(entries.add(index)) };
        if entry.vendor_guid == ACPI_TABLE_GUID {
            return Some(entry.vendor_table);
        }
    }
    None
}

#[cfg(test)]
mod acpi_rsdp_tests {
    use super::acpi_rsdp;
    use crate::guid::Guid;
    use crate::types::SystemTable;
    use core::ffi::c_void;

    /// ACPI 2.0 的配置表 GUID（`8868e871-e4f1-11d3-bc22-0080c73c8881`）。
    const ACPI_GUID: Guid = Guid {
        data1: 0x8868_e871,
        data2: 0xe4f1,
        data3: 0x11d3,
        data4: [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
    };
    const OTHER_GUID: Guid = Guid {
        data1: 0x1111_2222,
        data2: 0x3333,
        data3: 0x4444,
        data4: [0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc],
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Entry {
        guid: Guid,
        table: *mut c_void,
    }

    fn table_with(entries: &mut [Entry]) -> SystemTable {
        // SAFETY: 只取一个零值 SystemTable 并覆盖用到的两个字段。
        // 注意 `SystemTable` 含函数指针字段，故这里不用 `zeroed()`，而是逐字段构造。
        let mut table = unsafe { core::mem::MaybeUninit::<SystemTable>::zeroed().assume_init() };
        table.number_of_table_entries = entries.len();
        table.configuration_table = entries.as_mut_ptr() as *mut c_void;
        table
    }

    #[test]
    fn the_acpi_table_is_found_among_other_entries() {
        let mut rsdp = 0x1234_5678usize as *mut c_void;
        let mut entries = [
            Entry { guid: OTHER_GUID, table: core::ptr::null_mut() },
            Entry { guid: ACPI_GUID, table: rsdp },
        ];
        let table = table_with(&mut entries);
        let found = acpi_rsdp(&table).expect("必须找到 ACPI 表");
        assert_eq!(found, rsdp);
        let _ = &mut rsdp;
    }

    #[test]
    fn no_acpi_table_yields_none_instead_of_a_bogus_pointer() {
        let mut entries = [Entry { guid: OTHER_GUID, table: core::ptr::null_mut() }];
        let table = table_with(&mut entries);
        assert!(acpi_rsdp(&table).is_none(), "没有就返回 None，不编造");
    }
}