//! 固件卷上的文件：打开与读取。
//!
//! 边界：固件层只负责“按路径打开固件卷上的文件并读字节”；**文件系统解析**（EXT2/ISO9660/FAT）
//! 归 `crates/fs`，不在本层出现。句柄用抽象序号表示，不泄漏固件指针。

use crate::error::Error;

/// 文件句柄：固件侧不透明句柄的抽象序号。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileHandle(pub u32);

/// 固件文件信息。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileInfo {
    /// 文件大小（字节）。
    pub size: u64,
    /// 是否只读。
    pub read_only: bool,
}

/// 路径长度上限（字节）。
///
/// 取值理由：与常见固件路径缓冲上限同量级；超过即拒绝，避免各实现各自截断
/// （截断会静默打开错误的文件）。
pub const MAX_PATH_LEN: usize = 255;

/// 校验固件卷内的相对路径；各实现共用（单点定义）。
///
/// 规则：非空；不超过 [`MAX_PATH_LEN`]；不以 `/` 开头（相对固件卷根）；
/// 不出现空段、`.` 或 `..`（**拒绝而非归一化**：归一化会掩盖调用方错误）。
pub fn validate_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || path.len() > MAX_PATH_LEN {
        return Err(Error::InvalidArgument);
    }
    if path.starts_with('/') {
        return Err(Error::InvalidArgument);
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(Error::InvalidArgument);
        }
    }
    Ok(())
}

/// 固件卷文件访问：固件层能力 trait 之一。
pub trait FileSource {
    /// 按路径打开文件；路径非法返回 `InvalidArgument`，不存在返回 `NotFound`。
    fn open(&mut self, path: &str) -> Result<(FileHandle, FileInfo), Error>;

    /// 读入 `buffer`，返回实际读到的字节数（允许短读）。
    fn read(&mut self, handle: FileHandle, buffer: &mut [u8]) -> Result<usize, Error>;

    /// 关闭句柄。错误路径同样必须关闭（严格模式 S18：资源生命周期显式化）。
    fn close(&mut self, handle: FileHandle) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::{FileHandle, FileInfo, MAX_PATH_LEN, validate_path};
    use crate::error::Error;

    #[test]
    fn validate_path_rejects_empty_and_oversized_paths() {
        assert_eq!(validate_path(""), Err(Error::InvalidArgument));
        let long = "a".repeat(MAX_PATH_LEN + 1);
        assert_eq!(validate_path(&long), Err(Error::InvalidArgument));
        let exact = "a".repeat(MAX_PATH_LEN);
        assert_eq!(validate_path(&exact), Ok(()));
    }

    #[test]
    fn validate_path_rejects_absolute_paths() {
        assert_eq!(validate_path("/EFI/BOOT/BOOTX64.EFI"), Err(Error::InvalidArgument));
    }

    #[test]
    fn validate_path_rejects_empty_dot_and_dotdot_segments() {
        assert_eq!(validate_path("a//b"), Err(Error::InvalidArgument));
        assert_eq!(validate_path("a/./b"), Err(Error::InvalidArgument));
        assert_eq!(validate_path("a/../b"), Err(Error::InvalidArgument));
        assert_eq!(validate_path("a/"), Err(Error::InvalidArgument));
    }

    #[test]
    fn validate_path_accepts_relative_paths() {
        assert_eq!(validate_path("kernel"), Ok(()));
        assert_eq!(validate_path("EFI/BOOT/BOOTX64.EFI"), Ok(()));
        assert_eq!(validate_path("a/b/c"), Ok(()));
    }

    #[test]
    fn handle_and_info_are_value_types() {
        assert_eq!(FileHandle(0), FileHandle(0));
        assert_ne!(FileHandle(0), FileHandle(1));
        assert_eq!(FileInfo { size: 4096, read_only: true }.size, 4096);
    }
}
