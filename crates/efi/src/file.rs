//! `SimpleFileSystem` / `File` 协议与到固件抽象的映射。
//!
//! 边界：只声明**读取路径**所需的协议前缀。`EFI_FILE_PROTOCOL` 还有更多尾部成员
//! （写、定位、信息查询等），本实现不使用故**不声明**；实现只通过指针访问已声明字段，
//! 绝不按值复制整个结构（避免越界读取固件结构）。

use crate::types::Status;
use core::ffi::c_void;
use firmware::error::Error;

/// `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SimpleFileSystem {
    /// 协议修订号。
    pub revision: u64,
    /// 打开卷根目录。
    pub open_volume: OpenVolume,
}

/// `OpenVolume` 的签名。
pub type OpenVolume = unsafe extern "efiapi" fn(
    this: *mut SimpleFileSystem,
    root: *mut *mut FileProtocol,
) -> Status;

/// `EFI_FILE_PROTOCOL` 的**前缀**（读取路径所需字段）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FileProtocol {
    /// 协议修订号。
    pub revision: u64,
    /// 打开文件。
    pub open: *mut c_void,
    /// 关闭文件。
    pub close: Close,
    /// 删除文件（不使用）。
    pub delete: *mut c_void,
    /// 读取。
    pub read: Read,
}

/// `Close` 的签名。
pub type Close = unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status;

/// `Read` 的签名；`buffer_size` 入参为缓冲大小，出参为**实际读取字节数**（允许短读）。
pub type Read = unsafe extern "efiapi" fn(
    this: *mut FileProtocol,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Status;

/// 校验固件返回的读取字节数。
///
/// 契约：返回值不得超过调用方请求的缓冲大小；**超过即固件违反契约**，按 `Error::Io` 拒绝，
/// 绝不信任（否则会把越界数据当成有效内容）。
pub fn validate_read_result(requested: usize, returned: usize) -> Result<usize, Error> {
    if returned > requested {
        return Err(Error::Io);
    }
    Ok(returned)
}

#[cfg(test)]
mod tests {
    use super::{FileProtocol, SimpleFileSystem, validate_read_result};
    use core::mem::offset_of;
    use firmware::error::Error;

    #[test]
    fn simple_file_system_layout_matches_the_spec() {
        assert_eq!(offset_of!(SimpleFileSystem, revision), 0);
        assert_eq!(offset_of!(SimpleFileSystem, open_volume), 8);
    }

    #[test]
    fn file_protocol_prefix_offsets_match_the_spec() {
        assert_eq!(offset_of!(FileProtocol, revision), 0);
        assert_eq!(offset_of!(FileProtocol, open), 8);
        assert_eq!(offset_of!(FileProtocol, close), 16);
        assert_eq!(offset_of!(FileProtocol, delete), 24);
        assert_eq!(offset_of!(FileProtocol, read), 32);
    }

    #[test]
    fn read_result_must_not_exceed_the_requested_buffer() {
        assert_eq!(validate_read_result(512, 0), Ok(0));
        assert_eq!(validate_read_result(512, 512), Ok(512));
        assert_eq!(validate_read_result(512, 100), Ok(100));
        assert_eq!(validate_read_result(512, 513), Err(Error::Io));
    }
}
