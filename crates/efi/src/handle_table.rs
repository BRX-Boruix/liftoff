//! 文件句柄表：把固件指针映射为抽象序号。
//!
//! 边界：只做**映射与槽位管理**，不持有任何固件调用；容量固定，**无隐式分配**。

use crate::file::FileProtocol;
use firmware::error::Error;
use firmware::file::FileHandle;

/// 固定容量的文件句柄表（槽位可复用）。
pub struct HandleTable<'a> {
    slots: &'a mut [Option<*mut FileProtocol>],
}

impl<'a> HandleTable<'a> {
    /// 以调用方提供的槽位数组构造。
    pub const fn new(slots: &'a mut [Option<*mut FileProtocol>]) -> Self {
        Self { slots }
    }

    /// 登记一个固件句柄，返回抽象序号；表满返回 `Error::OutOfResources`。
    pub fn insert(&mut self, file: *mut FileProtocol) -> Result<FileHandle, Error> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file);
                return Ok(FileHandle(index as u32));
            }
        }
        Err(Error::OutOfResources)
    }

    /// 取出序号对应的固件句柄（不释放槽位）。
    pub fn get(&self, handle: FileHandle) -> Result<*mut FileProtocol, Error> {
        self.slots
            .get(handle.0 as usize)
            .and_then(|slot| *slot)
            .ok_or(Error::NotFound)
    }

    /// 释放槽位并返回其中的固件句柄。
    pub fn remove(&mut self, handle: FileHandle) -> Result<*mut FileProtocol, Error> {
        let slot = self
            .slots
            .get_mut(handle.0 as usize)
            .ok_or(Error::NotFound)?;
        slot.take().ok_or(Error::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::HandleTable;
    use crate::file::FileProtocol;
    use firmware::error::Error;
    use firmware::file::FileHandle;

    fn slot() -> Option<*mut FileProtocol> {
        None
    }

    #[test]
    fn insert_assigns_sequential_indices() {
        let mut slots = [slot(); 2];
        let mut table = HandleTable::new(&mut slots);
        let a = table.insert(0x1000usize as *mut FileProtocol).expect("插入");
        let b = table.insert(0x2000usize as *mut FileProtocol).expect("插入");
        assert_eq!(a, FileHandle(0));
        assert_eq!(b, FileHandle(1));
        assert_eq!(table.get(a), Ok(0x1000usize as *mut FileProtocol));
    }

    #[test]
    fn a_full_table_reports_out_of_resources() {
        let mut slots = [slot(); 1];
        let mut table = HandleTable::new(&mut slots);
        table.insert(0x1000usize as *mut FileProtocol).expect("插入");
        assert_eq!(table.insert(0x2000usize as *mut FileProtocol), Err(Error::OutOfResources));
    }

    #[test]
    fn removed_slots_are_reused_and_unknown_handles_are_rejected() {
        let mut slots = [slot(); 1];
        let mut table = HandleTable::new(&mut slots);
        let a = table.insert(0x1000usize as *mut FileProtocol).expect("插入");
        assert_eq!(table.remove(a), Ok(0x1000usize as *mut FileProtocol));
        assert_eq!(table.get(a), Err(Error::NotFound), "已释放的序号不可再用");
        assert_eq!(table.remove(a), Err(Error::NotFound));
        let b = table.insert(0x3000usize as *mut FileProtocol).expect("槽位应被复用");
        assert_eq!(b, FileHandle(0));
        assert_eq!(table.get(FileHandle(9)), Err(Error::NotFound));
    }
}
