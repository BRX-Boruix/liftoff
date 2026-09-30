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
