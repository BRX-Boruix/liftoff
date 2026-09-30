//! 内存映射的编排：把固件给出的原始描述符字节流转换为抽象条目。
//!
//! 边界：本模块只做**转换与容量校验**；实际的 `GetMemoryMap` 调用在下一步以可注入的
//! 函数指针形式接入（这样整条编排可在宿主上测透）。

use crate::memory::{MemoryDescriptor, descriptor_to_entry};
use core::mem::size_of;
use firmware::error::Error;
use firmware::memory::MemoryEntry;

/// 把 `raw` 中的 UEFI 描述符逐条转换为抽象条目并写入 `buffer`，返回条目数。
///
/// - `descriptor_size` 小于 `MemoryDescriptor` 大小 → `Error::InvalidArgument`
///   （连字段都读不全，不能猜）；
/// - 完整描述符数超过 `buffer` 容量 → `Error::BufferTooSmall`；
/// - 尾部不足一个描述符的字节**忽略**（固件允许缓冲比描述符数略大）。
pub fn descriptors_to_entries(
    raw: &[u8],
    descriptor_size: usize,
    buffer: &mut [MemoryEntry],
) -> Result<usize, Error> {
    if descriptor_size < size_of::<MemoryDescriptor>() {
        return Err(Error::InvalidArgument);
    }
    let count = raw.len() / descriptor_size;
    if count > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    for (index, slot) in buffer.iter_mut().take(count).enumerate() {
        let offset = index * descriptor_size;
        // SAFETY: `count = raw.len() / descriptor_size` 且 `descriptor_size >= 结构大小`，
        // 故 [offset, offset + size_of::<MemoryDescriptor>()) 必定落在 `raw` 内；
        // 使用 `read_unaligned` 是因为描述符在缓冲中不保证 8 字节对齐。
        let descriptor = unsafe {
            core::ptr::read_unaligned(raw.as_ptr().add(offset).cast::<MemoryDescriptor>())
        };
        *slot = descriptor_to_entry(&descriptor)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::descriptors_to_entries;
    use crate::memory::MemoryDescriptor;
    use firmware::error::Error;
    use firmware::memory::{MemoryEntry, MemoryKind};

    fn raw(entries: &[(u32, u64, u64)]) -> std::vec::Vec<u8> {
        let mut bytes = std::vec::Vec::new();
        for (kind, start, pages) in entries {
            let d = MemoryDescriptor {
                memory_type: *kind,
                pad: 0,
                physical_start: *start,
                virtual_start: 0,
                number_of_pages: *pages,
                attribute: 0,
            };
            // SAFETY: 只把已初始化的值按字节读出，用于构造测试输入。
            let slice = unsafe {
                core::slice::from_raw_parts(
                    (&d as *const MemoryDescriptor).cast::<u8>(),
                    core::mem::size_of::<MemoryDescriptor>(),
                )
            };
            bytes.extend_from_slice(slice);
        }
        bytes
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry { base: arch::addr::PhysAddr::new(0), length: 0, kind: MemoryKind::Reserved }
    }

    #[test]
    fn descriptors_are_converted_in_order() {
        let bytes = raw(&[(7, 0x1000, 2), (9, 0x9000, 1)]);
        let mut buffer = [empty_entry(); 4];
        let count = descriptors_to_entries(&bytes, 40, &mut buffer).expect("转换成功");
        assert_eq!(count, 2);
        assert_eq!(buffer[0].kind, MemoryKind::Usable);
        assert_eq!(buffer[0].length, 8192);
        assert_eq!(buffer[1].kind, MemoryKind::AcpiReclaimable);
    }

    #[test]
    fn trailing_partial_descriptor_is_ignored() {
        let mut bytes = raw(&[(7, 0x1000, 1)]);
        bytes.extend_from_slice(&[0u8; 7]);
        let mut buffer = [empty_entry(); 4];
        let count = descriptors_to_entries(&bytes, 40, &mut buffer).expect("转换成功");
        assert_eq!(count, 1);
    }

    #[test]
    fn descriptor_size_smaller_than_the_struct_is_rejected() {
        let bytes = raw(&[(7, 0x1000, 1)]);
        let mut buffer = [empty_entry(); 4];
        assert_eq!(descriptors_to_entries(&bytes, 39, &mut buffer), Err(Error::InvalidArgument));
    }

    #[test]
    fn too_small_output_buffer_is_reported() {
        let bytes = raw(&[(7, 0x1000, 1), (7, 0x2000, 1)]);
        let mut buffer = [empty_entry(); 1];
        assert_eq!(descriptors_to_entries(&bytes, 40, &mut buffer), Err(Error::BufferTooSmall));
    }
}
