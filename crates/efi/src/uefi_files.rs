//! `firmware::file::FileSource` 的 UEFI 实现。
//!
//! 边界：所有缓冲由调用方提供（句柄槽位、宽字符缓冲、信息缓冲），**无隐式分配**；
//! 失败路径一律关闭已打开的固件句柄（S18）。

use crate::file::{Close, FileProtocol, OpenVolume, Read, SimpleFileSystem};
use crate::file_info::GetInfo;
use crate::file_io::{close_once, read_once};
use crate::file_open::Open;
use crate::file_open_info::open_with_info;
use crate::handle_table::HandleTable;
use crate::volume::open_volume_root;
use firmware::error::Error;
use firmware::file::{FileHandle, FileInfo, FileSource};

/// 基于 UEFI 简单文件系统的文件来源。
pub struct UefiFiles<'a> {
    root: *mut FileProtocol,
    open: Open,
    read: Read,
    close: Close,
    get_info: GetInfo,
    handles: HandleTable<'a>,
    wide: &'a mut [u16],
    info_buffer: &'a mut [u8],
}

impl<'a> UefiFiles<'a> {
    /// 打开卷根并构造。
    pub fn new(
        open_volume: OpenVolume,
        file_system: *mut SimpleFileSystem,
        open: Open,
        read: Read,
        close: Close,
        get_info: GetInfo,
        slots: &'a mut [Option<*mut FileProtocol>],
        wide: &'a mut [u16],
        info_buffer: &'a mut [u8],
    ) -> Result<Self, Error> {
        let root = open_volume_root(open_volume, file_system)?;
        Ok(Self {
            root,
            open,
            read,
            close,
            get_info,
            handles: HandleTable::new(slots),
            wide,
            info_buffer,
        })
    }
}

impl FileSource for UefiFiles<'_> {
    fn open(&mut self, path: &str) -> Result<(FileHandle, FileInfo), Error> {
        let (file, info) = open_with_info(
            self.root,
            self.open,
            self.close,
            self.get_info,
            path,
            self.wide,
            self.info_buffer,
        )?;
        match self.handles.insert(file) {
            Ok(handle) => Ok((handle, info)),
            Err(err) => {
                // 句柄表满：同样必须关闭，否则泄漏（S18）。
                let _ = close_once(self.close, file);
                Err(err)
            }
        }
    }

    fn read(&mut self, handle: FileHandle, buffer: &mut [u8]) -> Result<usize, Error> {
        let file = self.handles.get(handle)?;
        read_once(self.read, file, buffer)
    }

    fn close(&mut self, handle: FileHandle) -> Result<(), Error> {
        // 先释放槽位再关闭：即使固件关闭失败，句柄也不应留在表里（否则后续会重复关闭）。
        let file = self.handles.remove(handle)?;
        close_once(self.close, file)
    }
}

#[cfg(test)]
mod tests {
    use super::UefiFiles;
    use crate::file::{Close, FileProtocol, OpenVolume, Read, SimpleFileSystem};
    use crate::file_info::{FileInfo as EfiFileInfo, GetInfo, Time};
    use crate::file_open::Open;
    use crate::guid::Guid;
    use crate::types::{Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;
    use firmware::file::{FileHandle, FileSource};

    static CLOSED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_open_volume(
        _t: *mut SimpleFileSystem,
        root: *mut *mut FileProtocol,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针。
        unsafe { *root = 0x10usize as *mut FileProtocol };
        SUCCESS
    }

    unsafe extern "efiapi" fn fake_open(
        _t: *mut FileProtocol,
        h: *mut *mut FileProtocol,
        _n: *mut u16,
        _m: u64,
        _a: u64,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针。
        unsafe { *h = 0x20usize as *mut FileProtocol };
        SUCCESS
    }

    unsafe extern "efiapi" fn fake_read(
        _t: *mut FileProtocol,
        size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针。
        unsafe {
            *size = 4;
            core::ptr::write_bytes(buffer.cast::<u8>(), 0x9A, 4)
        };
        SUCCESS
    }

    unsafe extern "efiapi" fn fake_close(_t: *mut FileProtocol) -> Status {
        CLOSED.fetch_add(1, Ordering::SeqCst);
        SUCCESS
    }

    unsafe extern "efiapi" fn fake_get_info(
        _t: *mut FileProtocol,
        _g: *const Guid,
        size: *mut usize,
        buffer: *mut c_void,
    ) -> Status {
        // SAFETY: 调用方按契约传入有效指针。
        unsafe {
            if buffer.is_null() {
                *size = 80;
                crate::types::BUFFER_TOO_SMALL
            } else {
                let t = Time {
                    year: 2026,
                    month: 9,
                    day: 30,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    pad1: 0,
                    nanosecond: 0,
                    timezone: 0,
                    daylight: 0,
                    pad2: 0,
                };
                *size = 80;
                core::ptr::write_unaligned(
                    buffer.cast::<EfiFileInfo>(),
                    EfiFileInfo {
                        size: 80,
                        file_size: 1234,
                        physical_size: 4096,
                        create_time: t,
                        last_access_time: t,
                        modification_time: t,
                        attribute: 0,
                    },
                );
                SUCCESS
            }
        }
    }

    fn ov() -> OpenVolume {
        fake_open_volume
    }
    fn op() -> Open {
        fake_open
    }
    fn rd() -> Read {
        fake_read
    }
    fn cl() -> Close {
        fake_close
    }
    fn gi() -> GetInfo {
        fake_get_info
    }

    #[test]
    fn open_read_close_works_end_to_end() {
        CLOSED.store(0, Ordering::SeqCst);
        let mut slots = [None; 2];
        let mut wide = [0u16; 32];
        let mut info_buffer = [0u8; 128];
        let mut files = UefiFiles::new(
            ov(),
            core::ptr::null_mut(),
            op(),
            rd(),
            cl(),
            gi(),
            &mut slots,
            &mut wide,
            &mut info_buffer,
        )
        .expect("卷根可打开");
        let (handle, info) = files.open("kernel").expect("打开");
        assert_eq!(info.size, 1234);
        let mut buffer = [0u8; 8];
        assert_eq!(files.read(handle, &mut buffer).expect("读"), 4);
        assert_eq!(buffer[0], 0x9A);
        files.close(handle).expect("关闭");
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn closing_frees_the_slot_and_unknown_handles_are_rejected() {
        CLOSED.store(0, Ordering::SeqCst);
        let mut slots = [None; 2];
        let mut wide = [0u16; 32];
        let mut info_buffer = [0u8; 128];
        let mut files = UefiFiles::new(
            ov(),
            core::ptr::null_mut(),
            op(),
            rd(),
            cl(),
            gi(),
            &mut slots,
            &mut wide,
            &mut info_buffer,
        )
        .expect("卷根可打开");
        let (handle, _) = files.open("a").expect("打开");
        files.close(handle).expect("首次关闭成功");
        assert_eq!(files.close(handle), Err(Error::NotFound), "已释放的句柄不可再关闭");
        let mut buffer = [0u8; 8];
        assert_eq!(files.read(FileHandle(9), &mut buffer), Err(Error::NotFound));
    }
}
