//! `EFI_BOOT_SERVICES` 表前缀。
//!
//! 边界：只声明到 `ExitBootServices` 的前缀；**未使用的服务保持 `*mut c_void` 占位**，
//! 以保证后续字段偏移正确 —— 这里写错一个字段就是运行期跳到错误地址。
//! 偏移由宿主单测断言。

use crate::boot_services::GetMemoryMap;
use crate::types::{Handle, Status, TableHeader};
use core::ffi::c_void;

/// `ExitBootServices` 的签名。
pub type ExitBootServices = unsafe extern "efiapi" fn(image_handle: Handle, map_key: usize) -> Status;

/// `HandleProtocol` 的签名。
pub type HandleProtocol = unsafe extern "efiapi" fn(
    handle: Handle,
    protocol: *const c_void,
    interface: *mut *mut c_void,
) -> Status;

/// `LocateHandle` 的签名。
pub type LocateHandle = unsafe extern "efiapi" fn(
    search_type: u32,
    protocol: *const c_void,
    search_key: *mut c_void,
    buffer_size: *mut usize,
    buffer: *mut Handle,
) -> Status;

/// `EFI_BOOT_SERVICES` 的前缀。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootServicesTable {
    /// 表头。
    pub hdr: TableHeader,
    /// 提升任务优先级（未使用）。
    pub raise_tpl: *mut c_void,
    /// 恢复任务优先级（未使用）。
    pub restore_tpl: *mut c_void,
    /// 分配页（未使用）。
    pub allocate_pages: *mut c_void,
    /// 释放页（未使用）。
    pub free_pages: *mut c_void,
    /// 取内存映射。
    pub get_memory_map: GetMemoryMap,
    /// 分配池内存（未使用）。
    pub allocate_pool: *mut c_void,
    /// 释放池内存（未使用）。
    pub free_pool: *mut c_void,
    /// 创建事件（未使用）。
    pub create_event: *mut c_void,
    /// 设置定时器（未使用）。
    pub set_timer: *mut c_void,
    /// 等待事件（未使用）。
    pub wait_for_event: *mut c_void,
    /// 触发事件（未使用）。
    pub signal_event: *mut c_void,
    /// 关闭事件（未使用）。
    pub close_event: *mut c_void,
    /// 检查事件（未使用）。
    pub check_event: *mut c_void,
    /// 安装协议接口（未使用）。
    pub install_protocol_interface: *mut c_void,
    /// 重装协议接口（未使用）。
    pub reinstall_protocol_interface: *mut c_void,
    /// 卸载协议接口（未使用）。
    pub uninstall_protocol_interface: *mut c_void,
    /// 查询句柄上的协议。
    pub handle_protocol: HandleProtocol,
    /// 保留字段。
    pub reserved: *mut c_void,
    /// 注册协议通知（未使用）。
    pub register_protocol_notify: *mut c_void,
    /// 按协议查找句柄。
    pub locate_handle: LocateHandle,
    /// 按设备路径查找（未使用）。
    pub locate_device_path: *mut c_void,
    /// 安装配置表（未使用）。
    pub install_configuration_table: *mut c_void,
    /// 装载映像（未使用）。
    pub load_image: *mut c_void,
    /// 启动映像（未使用）。
    pub start_image: *mut c_void,
    /// 退出当前映像（未使用）。
    pub exit: *mut c_void,
    /// 卸载映像（未使用）。
    pub unload_image: *mut c_void,
    /// 退出引导服务。
    pub exit_boot_services: ExitBootServices,
}

#[cfg(test)]
mod tests {
    use super::BootServicesTable;
    use core::mem::offset_of;

    #[test]
    fn memory_and_exit_offsets_match_the_spec() {
        assert_eq!(offset_of!(BootServicesTable, hdr), 0);
        assert_eq!(offset_of!(BootServicesTable, raise_tpl), 24);
        assert_eq!(offset_of!(BootServicesTable, allocate_pages), 40);
        assert_eq!(offset_of!(BootServicesTable, free_pages), 48);
        assert_eq!(offset_of!(BootServicesTable, get_memory_map), 56);
        assert_eq!(offset_of!(BootServicesTable, allocate_pool), 64);
        assert_eq!(offset_of!(BootServicesTable, free_pool), 72);
    }

    #[test]
    fn protocol_and_exit_offsets_match_the_spec() {
        assert_eq!(offset_of!(BootServicesTable, handle_protocol), 152);
        assert_eq!(offset_of!(BootServicesTable, locate_handle), 176);
        assert_eq!(offset_of!(BootServicesTable, exit_boot_services), 232);
    }
}
