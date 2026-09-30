//! UEFI 返回码 → 抽象错误。
//!
//! 保守映射：只把**语义明确**的返回码映射到具体错误；其余错误一律 `Error::Io`
//! （不假装知道具体原因；原始返回码由调用方在诊断时另行记录）。

use crate::types::{BUFFER_TOO_SMALL, DEVICE_ERROR, INVALID_PARAMETER, NOT_FOUND, SUCCESS, Status, UNSUPPORTED};
use firmware::error::Error;

/// 返回码 → 错误；`SUCCESS` 返回 `None`。
pub const fn status_to_error(status: Status) -> Option<Error> {
    let code = status.as_usize();
    if code == SUCCESS.as_usize() {
        None
    } else if code == NOT_FOUND.as_usize() {
        Some(Error::NotFound)
    } else if code == UNSUPPORTED.as_usize() {
        Some(Error::Unsupported)
    } else if code == BUFFER_TOO_SMALL.as_usize() {
        Some(Error::BufferTooSmall)
    } else if code == INVALID_PARAMETER.as_usize() {
        Some(Error::InvalidArgument)
    } else if code == DEVICE_ERROR.as_usize() {
        Some(Error::Io)
    } else {
        Some(Error::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::status_to_error;
    use crate::types::{BUFFER_TOO_SMALL, DEVICE_ERROR, INVALID_PARAMETER, NOT_FOUND, SUCCESS, Status, UNSUPPORTED};
    use firmware::error::Error;

    #[test]
    fn success_maps_to_no_error() {
        assert_eq!(status_to_error(SUCCESS), None);
    }

    #[test]
    fn known_statuses_map_to_specific_errors() {
        assert_eq!(status_to_error(NOT_FOUND), Some(Error::NotFound));
        assert_eq!(status_to_error(UNSUPPORTED), Some(Error::Unsupported));
        assert_eq!(status_to_error(BUFFER_TOO_SMALL), Some(Error::BufferTooSmall));
        assert_eq!(status_to_error(INVALID_PARAMETER), Some(Error::InvalidArgument));
    }

    #[test]
    fn unknown_error_statuses_fall_back_to_io() {
        assert_eq!(status_to_error(DEVICE_ERROR), Some(Error::Io));
        assert_eq!(status_to_error(Status(0x8000_0000_0000_00FF)), Some(Error::Io));
    }
}
