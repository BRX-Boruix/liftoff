//! Limine 可执行文件协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应是 { u64 revision; limine_file *executable_file; }，共 16 字节。

use crate::base::COMMON_MAGIC;
use crate::file::File;

/// `LIMINE_EXECUTABLE_FILE_REQUEST_ID`。
pub const EXECUTABLE_FILE_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0xad97e90e83f1ed67,
    0x31eb5d1c5ff23b69,
];

/// `struct limine_executable_file_response`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExecutableFileResponse {
    /// 响应修订。
    pub revision: u64,
    /// 内核文件描述。
    pub executable_file: *mut File,
}

/// `struct limine_executable_file_request`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExecutableFileRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut ExecutableFileResponse,
}

#[cfg(test)]
mod tests {
    use super::{EXECUTABLE_FILE_REQUEST_ID, ExecutableFileRequest, ExecutableFileResponse};
    use core::mem::{offset_of, size_of};

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<ExecutableFileResponse>(), 16);
        assert_eq!(offset_of!(ExecutableFileResponse, revision), 0);
        assert_eq!(offset_of!(ExecutableFileResponse, executable_file), 8);
    }

    #[test]
    fn request_layout_matches_the_header() {
        assert_eq!(size_of::<ExecutableFileRequest>(), 48);
        assert_eq!(offset_of!(ExecutableFileRequest, response), 40);
    }

    #[test]
    fn request_id_matches_the_header() {
        assert_eq!(EXECUTABLE_FILE_REQUEST_ID[2], 0xad97e90e83f1ed67);
        assert_eq!(EXECUTABLE_FILE_REQUEST_ID[3], 0x31eb5d1c5ff23b69);
    }
}
