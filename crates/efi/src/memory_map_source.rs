//! 两段式 `GetMemoryMap` 编排。
//!
//! 编排：① 以 0 大小探测（固件回 `BUFFER_TOO_SMALL` 并写回所需字节数与描述符大小）；
//! ② 用 `descriptor_capacity` 校验调用方描述符缓冲是否够大；③ 取数据；④ 逐条转换。
//!
//! `GetMemoryMap` 以参数注入，因此整条编排可在宿主上用假固件测透。

use crate::boot_services::{GetMemoryMap, descriptor_capacity};
use crate::memory_map::descriptors_to_entries;
use crate::status::status_to_error;
use crate::types::{BUFFER_TOO_SMALL, SUCCESS};
use core::ffi::c_void;
use firmware::error::Error;
use firmware::memory::MemoryEntry;

/// 加载内存映射，返回 `(条目数, map_key)`。
///
/// `map_key` 必须在退出引导服务时原样传回。
pub fn load_memory_map(
    get_memory_map: GetMemoryMap,
    descriptors: &mut [u8],
    buffer: &mut [MemoryEntry],
) -> Result<(usize, usize), Error> {
    let mut needed: usize = 0;
    let mut probe_key: usize = 0;
    let mut probe_size: usize = 0;
    let mut probe_version: u32 = 0;
    // SAFETY: 探测用法 —— map 为 null 且 map_size 为 0；其余指针指向本栈上的有效变量。
    let probe = unsafe {
        get_memory_map(
            &mut needed,
            core::ptr::null_mut(),
            &mut probe_key,
            &mut probe_size,
            &mut probe_version,
        )
    };
    if probe != SUCCESS && probe != BUFFER_TOO_SMALL {
        return Err(status_to_error(probe).unwrap_or(Error::Io));
    }
    let capacity = descriptor_capacity(needed, probe_size).ok_or(Error::Io)?;
    // 单位必须各自对齐：条目数比条目缓冲，字节数比字节缓冲。
    if capacity > buffer.len() {
        return Err(Error::BufferTooSmall);
    }
    if needed > descriptors.len() {
        return Err(Error::BufferTooSmall);
    }
    let mut size = descriptors.len();
    let mut key: usize = 0;
    let mut descriptor_size: usize = 0;
    let mut version: u32 = 0;
    // SAFETY: `descriptors` 是可写缓冲且 `size` 为其长度；其余指针指向本栈上的有效变量。
    let taken = unsafe {
        get_memory_map(
            &mut size,
            descriptors.as_mut_ptr().cast::<c_void>(),
            &mut key,
            &mut descriptor_size,
            &mut version,
        )
    };
    if let Some(err) = status_to_error(taken) {
        return Err(err);
    }
    // 固件报告的字节数不得超过我们给的缓冲，否则切片会 panic（引导器里 panic 是致命的）。
    if size > descriptors.len() {
        return Err(Error::Io);
    }
    let count = descriptors_to_entries(&descriptors[..size], descriptor_size, buffer)?;
    Ok((count, key))
}

#[cfg(test)]
mod tests {
    use super::load_memory_map;
    use crate::memory::MemoryDescriptor;
    use crate::types::{BUFFER_TOO_SMALL, Status, SUCCESS};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;
    use firmware::memory::{MemoryEntry, MemoryKind};

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static DESCRIPTOR: MemoryDescriptor = MemoryDescriptor {
        memory_type: 7,
        pad: 0,
        physical_start: 0x1000,
        virtual_start: 0,
        number_of_pages: 2,
        attribute: 0,
    };

    /// 假固件：第一次调用（map 为空）报告所需大小；第二次把描述符写进缓冲。
    unsafe extern "efiapi" fn fake_get_memory_map(
        map_size: *mut usize,
        map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: 调用方按 UEFI 契约传入有效指针；测试中始终如此。
        unsafe {
            *map_key = 0x1234;
            *descriptor_size = 40;
            if map.is_null() {
                *map_size = 40;
                BUFFER_TOO_SMALL
            } else {
                *map_size = 40;
                core::ptr::write_unaligned(map.cast::<MemoryDescriptor>(), DESCRIPTOR);
                SUCCESS
            }
        }
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry { base: arch::addr::PhysAddr::new(0), length: 0, kind: MemoryKind::Reserved }
    }

    #[test]
    fn two_stage_load_converts_the_map_and_returns_the_key() {
        CALLS.store(0, Ordering::SeqCst);
        let mut descriptors = [0u8; 128];
        let mut buffer = [empty_entry(); 4];
        let (count, key) = load_memory_map(fake_get_memory_map, &mut descriptors, &mut buffer)
            .expect("两段式加载成功");
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "应当探测一次、取数据一次");
        assert_eq!(count, 1);
        assert_eq!(key, 0x1234);
        assert_eq!(buffer[0].kind, MemoryKind::Usable);
        assert_eq!(buffer[0].length, 8192);
    }

    #[test]
    fn too_small_descriptor_buffer_stops_after_the_probe() {
        CALLS.store(0, Ordering::SeqCst);
        let mut descriptors = [0u8; 8];
        let mut buffer = [empty_entry(); 4];
        assert_eq!(
            load_memory_map(fake_get_memory_map, &mut descriptors, &mut buffer),
            Err(Error::BufferTooSmall)
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "容量不足时不应发起第二次调用");
    }
}
