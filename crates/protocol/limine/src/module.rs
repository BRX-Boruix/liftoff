//! Limine 模块协议。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! 响应 24 字节；请求 64 字节（修订 1 追加 internal_module_count/internal_modules）；
//! internal_module 是 { const char *path; const char *string; u64 flags; }，24 字节。

use crate::base::COMMON_MAGIC;
use crate::file::File;

/// `LIMINE_MODULE_REQUEST_ID`。
pub const MODULE_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x3e7e279702be32af,
    0xca1c4f3bd1280cee,
];

/// `LIMINE_INTERNAL_MODULE_REQUIRED`。
pub const INTERNAL_MODULE_REQUIRED: u64 = 1;

/// `LIMINE_INTERNAL_MODULE_COMPRESSED`。
pub const INTERNAL_MODULE_COMPRESSED: u64 = 2;

/// `struct limine_internal_module`（内核侧声明的模块条目）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InternalModule {
    /// 路径。
    pub path: *const core::ffi::c_char,
    /// 命令行/字符串。
    pub string: *const core::ffi::c_char,
    /// 标志（见 `INTERNAL_MODULE_*`）。
    pub flags: u64,
}

/// `struct limine_module_response`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ModuleResponse {
    /// 响应修订。
    pub revision: u64,
    /// 模块数量。
    pub module_count: u64,
    /// 指向模块文件指针数组的指针。
    pub modules: *mut *mut File,
}

/// `struct limine_module_request`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ModuleRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut ModuleResponse,
    /// 内核内建模块数量（请求修订 1）。
    pub internal_module_count: u64,
    /// 内核内建模块数组（双重指针）。
    pub internal_modules: *mut *mut InternalModule,
}

#[cfg(test)]
mod tests {
    use super::{
        INTERNAL_MODULE_COMPRESSED, INTERNAL_MODULE_REQUIRED, InternalModule, MODULE_REQUEST_ID,
        ModuleRequest, ModuleResponse,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn internal_module_layout_matches_the_header() {
        assert_eq!(size_of::<InternalModule>(), 24);
        assert_eq!(offset_of!(InternalModule, path), 0);
        assert_eq!(offset_of!(InternalModule, string), 8);
        assert_eq!(offset_of!(InternalModule, flags), 16);
    }

    #[test]
    fn response_layout_matches_the_header() {
        assert_eq!(size_of::<ModuleResponse>(), 24);
        assert_eq!(offset_of!(ModuleResponse, revision), 0);
        assert_eq!(offset_of!(ModuleResponse, module_count), 8);
        assert_eq!(offset_of!(ModuleResponse, modules), 16);
    }

    #[test]
    fn request_layout_includes_the_revision_1_fields() {
        assert_eq!(size_of::<ModuleRequest>(), 64);
        assert_eq!(offset_of!(ModuleRequest, response), 40);
        assert_eq!(offset_of!(ModuleRequest, internal_module_count), 48);
        assert_eq!(offset_of!(ModuleRequest, internal_modules), 56);
    }

    #[test]
    fn id_and_flags_match_the_header() {
        assert_eq!(MODULE_REQUEST_ID[2], 0x3e7e279702be32af);
        assert_eq!(MODULE_REQUEST_ID[3], 0xca1c4f3bd1280cee);
        assert_eq!(INTERNAL_MODULE_REQUIRED, 1);
        assert_eq!(INTERNAL_MODULE_COMPRESSED, 2);
    }
}
