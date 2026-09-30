//! `firmware::memory::MemoryMapSource` 的 UEFI 实现（第一个真实实现）。
//!
//! 边界：**无隐式分配** —— 描述符暂存缓冲由调用方提供并随本结构借用；
//! `map_key` 由本结构保存，供退出引导服务时原样传回。

use crate::boot_services::GetMemoryMap;
use crate::memory_map_source::load_memory_map;
use firmware::error::Error;
use firmware::memory::{MemoryEntry, MemoryMap, MemoryMapSource};

/// 基于 UEFI `GetMemoryMap` 的内存映射来源。
pub struct UefiMemoryMapSource<'a> {
    get_memory_map: GetMemoryMap,
    descriptors: &'a mut [u8],
    map_key: Option<usize>,
}

impl<'a> UefiMemoryMapSource<'a> {
    /// 以 `GetMemoryMap` 函数指针与调用方持有的描述符暂存缓冲构造。
    pub fn new(get_memory_map: GetMemoryMap, descriptors: &'a mut [u8]) -> Self {
        Self { get_memory_map, descriptors, map_key: None }
    }

    /// 最近一次成功加载得到的 `map_key`；尚未加载时为 `None`。
    pub const fn map_key(&self) -> Option<usize> {
        self.map_key
    }
}

impl MemoryMapSource for UefiMemoryMapSource<'_> {
    fn memory_map<'b>(&mut self, buffer: &'b mut [MemoryEntry]) -> Result<MemoryMap<'b>, Error> {
        let (count, key) = load_memory_map(self.get_memory_map, self.descriptors, buffer)?;
        self.map_key = Some(key);
        Ok(MemoryMap::new(&buffer[..count]))
    }
}

#[cfg(test)]
mod tests {
    use super::UefiMemoryMapSource;
    use crate::memory::MemoryDescriptor;
    use crate::types::{BUFFER_TOO_SMALL, Status, SUCCESS};
    use core::ffi::c_void;
    use firmware::memory::{MemoryEntry, MemoryKind, MemoryMapSource};

    static DESCRIPTOR: MemoryDescriptor = MemoryDescriptor {
        memory_type: 7,
        pad: 0,
        physical_start: 0x2000,
        virtual_start: 0,
        number_of_pages: 1,
        attribute: 0,
    };

    unsafe extern "efiapi" fn fake(
        map_size: *mut usize,
        map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        // SAFETY: 调用方按 UEFI 契约传入有效指针；测试中始终如此。
        unsafe {
            *map_key = 0x5678;
            *descriptor_size = 40;
            *map_size = 40;
            if map.is_null() {
                BUFFER_TOO_SMALL
            } else {
                core::ptr::write_unaligned(map.cast::<MemoryDescriptor>(), DESCRIPTOR);
                SUCCESS
            }
        }
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry { base: arch::addr::PhysAddr::new(0), length: 0, kind: MemoryKind::Reserved }
    }

    #[test]
    fn source_implements_the_abstract_trait_and_records_the_key() {
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(fake, &mut descriptors);
        assert_eq!(source.map_key(), None, "加载前没有 map_key");
        let mut buffer = [empty_entry(); 4];
        let map = source.memory_map(&mut buffer).expect("映射可取");
        assert_eq!(map.len(), 1);
        assert_eq!(map.entries()[0].kind, MemoryKind::Usable);
        assert_eq!(map.entries()[0].length, 4096);
        assert_eq!(source.map_key(), Some(0x5678), "必须保存 map_key 供退出引导服务使用");
    }

    #[test]
    fn source_reports_buffer_too_small_through_the_trait() {
        let mut descriptors = [0u8; 8];
        let mut source = UefiMemoryMapSource::new(fake, &mut descriptors);
        let mut buffer = [empty_entry(); 4];
        assert!(source.memory_map(&mut buffer).is_err());
        assert_eq!(source.map_key(), None, "失败时不得记录 map_key");
    }
}
