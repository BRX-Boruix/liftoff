//! 固件操作的统一失败原因。
//!
//! 失败模式先于正常路径定义（严格模式 S20）：每一类失败在此显式列出，实现**不得**用
//! 返回伪数据代替错误（S09）。所有能力 trait 共用本类型，避免各写一套错误。

/// 固件操作的失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// 设备或文件不存在。
    NotFound,
    /// 固件不支持该操作。
    Unsupported,
    /// 传输或设备错误。
    Io,
    /// 调用方提供的缓冲过小。
    BufferTooSmall,
    /// 固件资源耗尽（分配失败等）。
    OutOfResources,
    /// 参数不合法（对齐、越界等）。
    InvalidArgument,
    /// 当前状态不允许该操作（例如已退出引导服务）。
    InvalidState,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::NotFound => "设备或文件不存在",
            Self::Unsupported => "固件不支持该操作",
            Self::Io => "传输或设备错误",
            Self::BufferTooSmall => "调用方提供的缓冲过小",
            Self::OutOfResources => "固件资源耗尽",
            Self::InvalidArgument => "参数不合法",
            Self::InvalidState => "当前状态不允许该操作",
        };
        f.write_str(text)
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn every_failure_mode_has_a_message() {
        let cases = [
            (Error::NotFound, "设备或文件不存在"),
            (Error::Unsupported, "固件不支持该操作"),
            (Error::Io, "传输或设备错误"),
            (Error::BufferTooSmall, "调用方提供的缓冲过小"),
            (Error::OutOfResources, "固件资源耗尽"),
            (Error::InvalidArgument, "参数不合法"),
            (Error::InvalidState, "当前状态不允许该操作"),
        ];
        for (err, want) in cases {
            assert_eq!(std::format!("{err}"), want);
        }
    }

    #[test]
    fn error_is_a_value_type_and_a_core_error() {
        assert_eq!(Error::Io, Error::Io);
        assert_ne!(Error::Io, Error::NotFound);
        let dynamic: &dyn core::error::Error = &Error::InvalidState;
        assert_eq!(std::format!("{dynamic}"), "当前状态不允许该操作");
    }
}
