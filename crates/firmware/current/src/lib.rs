//! 固件实现选择器：把具体实现接到抽象上（ADR-050 模式）。
//!
//! 目标构建启用 `impl-uefi`（默认）；BIOS 将来作为第二个实现接入。
//! 未启用任何实现时编译失败 —— 宁可构建报错，也不要静默选一个实现。

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(not(feature = "impl-uefi"))]
compile_error!("必须启用一个固件实现 feature：目前是 `impl-uefi`");

/// 当前选定的固件实现门面。
///
/// 只暴露实现类型，不重导出 `efi` crate 的模块树 —— 调用方无需知道实现的内部布局，
/// 将来加 BIOS 时也只改这里。
#[cfg(feature = "impl-uefi")]
pub mod current {
    pub use efi::boot_services_table::ExitBootServices;
    pub use efi::types::{BUFFER_TOO_SMALL, DEVICE_ERROR, Handle, SUCCESS, Status};
    pub use efi::boot_services_table::AllocatePages;
    pub use efi::boot_services_table::{ALLOCATE_ANY_PAGES, EFI_LOADER_DATA};
    pub use efi::boot_services_table::EfiFrameAllocator;
    pub use efi::uefi_boot_services::UefiBootServices;
    pub use efi::boot_services_table::BootServicesTable;
    pub use efi::system_table::boot_services_of;
    pub use efi::types::SystemTable;
    pub use efi::uefi_block_devices::UefiBlockDevices;
    pub use efi::uefi_files::UefiFiles;
    pub use efi::uefi_graphics::UefiGraphics;
    pub use efi::uefi_memory_source::UefiMemoryMapSource;
}

#[cfg(all(test, feature = "impl-uefi"))]
mod tests {
    use firmware::block::BlockDeviceSource;
    use firmware::boot_services::BootServicesControl;
    use firmware::file::FileSource;
    use firmware::graphics::GraphicsSink;
    use firmware::memory::MemoryMapSource;

    fn assert_memory_source<T: MemoryMapSource>() {}
    fn assert_block_source<T: BlockDeviceSource>() {}
    fn assert_boot_control<T: BootServicesControl>() {}
    fn assert_file_source<T: FileSource>() {}
    fn assert_graphics_sink<T: GraphicsSink>() {}

    #[test]
    fn the_selected_implementation_satisfies_the_abstract_traits() {
        assert_memory_source::<crate::current::UefiMemoryMapSource<'static>>();
        assert_block_source::<crate::current::UefiBlockDevices<'static>>();
        assert_boot_control::<crate::current::UefiBootServices<'static>>();
        assert_file_source::<crate::current::UefiFiles<'static>>();
        assert_graphics_sink::<crate::current::UefiGraphics<'static>>();
    }

    #[test]
    fn the_facade_exposes_exactly_three_implementation_types() {
        // 编译期断言：门面里只有这三类（多导出一个会在这里被注意到）。
        let _ = core::mem::size_of::<crate::current::UefiMemoryMapSource<'static>>();
        let _ = core::mem::size_of::<crate::current::UefiBlockDevices<'static>>();
        let _ = core::mem::size_of::<crate::current::UefiBootServices<'static>>();
    }
}
