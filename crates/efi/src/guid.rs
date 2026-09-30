//! UEFI GUID 与协议 GUID 常量。
//!
//! 表示法：`EFI_GUID` 是 `{ u32, u16, u16, [u8; 8] }`，**前三个字段在内存中按小端存放**；
//! 文本形式 `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` 的前三字段是十六进制书写。
//! 这是最容易写反的地方，故用字节级测试固定。

/// UEFI GUID。
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Guid {
    /// 第一字段（文本形式的前 8 位十六进制）。
    pub data1: u32,
    /// 第二字段（文本形式的第二组）。
    pub data2: u16,
    /// 第三字段（文本形式的第三组）。
    pub data3: u16,
    /// 后 8 字节（文本形式最后两组，原序）。
    pub data4: [u8; 8],
}

/// `EFI_BLOCK_IO_PROTOCOL`：`{964e5b21-6459-11d2-8e39-00a0c969723b}`。
pub const BLOCK_IO_PROTOCOL: Guid = Guid {
    data1: 0x964e_5b21,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`：`{964e5b22-6459-11d2-8e39-00a0c969723b}`。
pub const SIMPLE_FILE_SYSTEM_PROTOCOL: Guid = Guid {
    data1: 0x964e_5b22,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

#[cfg(test)]
mod tests {
    use super::{BLOCK_IO_PROTOCOL, Guid, SIMPLE_FILE_SYSTEM_PROTOCOL};
    use core::mem::{offset_of, size_of};

    fn bytes_of(guid: &Guid) -> [u8; 16] {
        // SAFETY: `Guid` 是 #[repr(C)] 的 16 字节 POD，按字节读取其表示是安全的。
        let raw = unsafe {
            core::slice::from_raw_parts((guid as *const Guid).cast::<u8>(), size_of::<Guid>())
        };
        let mut out = [0u8; 16];
        out.copy_from_slice(raw);
        out
    }

    #[test]
    fn guid_layout_matches_the_spec() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(offset_of!(Guid, data1), 0);
        assert_eq!(offset_of!(Guid, data2), 4);
        assert_eq!(offset_of!(Guid, data3), 6);
        assert_eq!(offset_of!(Guid, data4), 8);
    }

    #[test]
    fn block_io_guid_is_stored_little_endian() {
        // 文本形式 {964e5b21-6459-11d2-8e39-00a0c969723b}；前三字段按小端存放。
        assert_eq!(
            bytes_of(&BLOCK_IO_PROTOCOL),
            [0x21, 0x5b, 0x4e, 0x96, 0x59, 0x64, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]
        );
    }

    #[test]
    fn simple_file_system_guid_is_stored_little_endian() {
        // 文本形式 {964e5b22-6459-11d2-8e39-00a0c969723b}。
        assert_eq!(
            bytes_of(&SIMPLE_FILE_SYSTEM_PROTOCOL),
            [0x22, 0x5b, 0x4e, 0x96, 0x59, 0x64, 0xd2, 0x11, 0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]
        );
    }
}
