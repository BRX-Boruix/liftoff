//! Minimal UEFI bindings: only what the skeleton needs today.

pub type Handle = *mut core::ffi::c_void;
pub type Status = usize;

pub const SUCCESS: Status = 0;

/// EFI_SYSTEM_TABLE (UEFI 2.10, section 4.3). Fields are declared in order so
/// the layout matches the firmware ABI; unused members stay opaque pointers.
#[repr(C)]
pub struct SystemTable {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
    pub firmware_vendor: *mut u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub console_in: *mut core::ffi::c_void,
    pub console_out_handle: Handle,
    pub console_out: *mut core::ffi::c_void,
    pub std_err_handle: Handle,
    pub std_err: *mut core::ffi::c_void,
    pub runtime_services: *mut core::ffi::c_void,
    pub boot_services: *mut core::ffi::c_void,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut core::ffi::c_void,
}
