//! Limine 文件结构与 UUID。
//!
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）：
//! limine_uuid 是 { u32 a; u16 b; u16 c; u8 d[8]; }（16 字节）；
//! limine_file 共 112 字节（尾部三个 UUID 各 16 字节）。

/// `LIMINE_MEDIA_TYPE_GENERIC`。
pub const MEDIA_TYPE_GENERIC: u32 = 0;

/// `struct limine_uuid`。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Uuid {
    /// 第一段。
    pub a: u32,
    /// 第二段。
    pub b: u16,
    /// 第三段。
    pub c: u16,
    /// 后 8 字节。
    pub d: [u8; 8],
}

/// `struct limine_file`（内核与模块共用）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct File {
    /// 响应修订。
    pub revision: u64,
    /// 文件内容地址。
    pub address: *mut core::ffi::c_void,
    /// 文件大小（字节）。
    pub size: u64,
    /// 路径。
    pub path: *mut core::ffi::c_char,
    /// 命令行/字符串。
    pub string: *mut core::ffi::c_char,
    /// 介质类型（见 `MEDIA_TYPE_*`）。
    pub media_type: u32,
    /// 保留。
    pub unused: u32,
    /// TFTP 对端 IPv4。
    pub tftp_ipv4: [u8; 4],
    /// TFTP 端口。
    pub tftp_port: u32,
    /// 分区索引。
    pub partition_index: u32,
    /// MBR 磁盘标识。
    pub mbr_disk_id: u32,
    /// GPT 磁盘 UUID。
    pub gpt_disk_uuid: Uuid,
    /// GPT 分区 UUID。
    pub gpt_part_uuid: Uuid,
    /// 分区 UUID。
    pub part_uuid: Uuid,
}

#[cfg(test)]
mod tests {
    use super::{File, MEDIA_TYPE_GENERIC, Uuid};
    use core::mem::{offset_of, size_of};

    #[test]
    fn uuid_layout_matches_the_header() {
        assert_eq!(size_of::<Uuid>(), 16);
        assert_eq!(offset_of!(Uuid, a), 0);
        assert_eq!(offset_of!(Uuid, b), 4);
        assert_eq!(offset_of!(Uuid, c), 6);
        assert_eq!(offset_of!(Uuid, d), 8);
    }

    #[test]
    fn file_layout_matches_the_header() {
        assert_eq!(size_of::<File>(), 112);
        assert_eq!(offset_of!(File, revision), 0);
        assert_eq!(offset_of!(File, address), 8);
        assert_eq!(offset_of!(File, size), 16);
        assert_eq!(offset_of!(File, path), 24);
        assert_eq!(offset_of!(File, string), 32);
        assert_eq!(offset_of!(File, media_type), 40);
        assert_eq!(offset_of!(File, unused), 44);
        assert_eq!(offset_of!(File, tftp_ipv4), 48);
        assert_eq!(offset_of!(File, tftp_port), 52);
        assert_eq!(offset_of!(File, partition_index), 56);
        assert_eq!(offset_of!(File, mbr_disk_id), 60);
        assert_eq!(offset_of!(File, gpt_disk_uuid), 64);
        assert_eq!(offset_of!(File, gpt_part_uuid), 80);
        assert_eq!(offset_of!(File, part_uuid), 96);
    }

    #[test]
    fn media_type_generic_is_zero() {
        assert_eq!(MEDIA_TYPE_GENERIC, 0);
    }
}
