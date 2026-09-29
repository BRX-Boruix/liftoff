//! M8：Limine 模块请求（ModuleRequest）的引导器侧实现。
//!
//! **仅当内核映像声明了该请求标记时才加载**（`boruix::has_request`）：
//! BORUIX 内核不声明 → 不读文件、不分配内存、行为零变化（其 ADR-028
//! 「介质即系统 / 单源」架构不受影响）。
//!
//! 交付形态对齐 brxlimine-rs lib.rs 674/689：`ModuleResponse` + module_count
//! 个 `File` **指针**（ArrayPtr 语义）；`File.base` 是 HHDM 虚地址，内核按
//! Limine 语义直接解引用。全部内存取自 EfiLoaderData → 重转后的 memmap 里
//! 是 BootloaderReclaimable，EBS 后有效。

use crate::boruix;
use crate::config;
use crate::efi;
use crate::iso9660;
use crate::serial;

/// 模块数上限。
pub const MAX_MODULES: usize = 8;
/// 单条路径/命令行字符串上限（含 NUL 终止符）。
const MAX_STR: usize = 128;

/// 结构页内的 File 数组偏移（8 × 112B = 896B）。
const OFF_FILES: usize = 0x000;
/// 指针数组偏移（8 × 8B = 64B）。
const OFF_PTRS: usize = 0x400;
/// 字符串区偏移（8 × 2 × 128B = 2048B → 页内 0x500..0xD00）。
const OFF_STR: usize = 0x500;

const _: () = assert!(OFF_PTRS >= OFF_FILES + MAX_MODULES * 112);
const _: () = assert!(OFF_STR >= OFF_PTRS + MAX_MODULES * 8);
const _: () = assert!(OFF_STR + MAX_MODULES * 2 * MAX_STR <= 4096);

/// 加载结果（物理地址；`Handover` 后续持有）。
pub struct LoadedModules {
    pub count: usize,
    pub files: *mut boruix::File,
    pub ptrs: *mut *mut boruix::File,
}

impl LoadedModules {
    pub const EMPTY: LoadedModules = LoadedModules {
        count: 0,
        files: core::ptr::null_mut(),
        ptrs: core::ptr::null_mut(),
    };
}

/// 按 `config::MODULES` 从 ISO 卷读取模块，拷入 LoaderData 常驻区，
/// 填好 `File`（含路径/命令行字符串）与指针数组。返回加载结果。
///
/// 错误码：0xD0 数量超限 / 0xD1 结构页 / 0xD2 打开失败 / 0xD3 数据页 /
/// 0xD4 读失败 / 0xD5 长度不符 / 0xD6 字符串过长。
pub fn load_from_iso(
    bs: &efi::BootServices,
    vol: &iso9660::Volume,
    dev: &mut iso9660::UefiBlock,
) -> Result<LoadedModules, usize> {
    let specs = config::MODULES;
    if specs.len() > MAX_MODULES {
        return Err(0xD0);
    }
    if specs.is_empty() {
        return Ok(LoadedModules::EMPTY);
    }

    let mut meta: u64 = 0;
    let st = unsafe {
        (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, 1, &mut meta)
    };
    if efi::is_error(st) {
        return Err(0xD1);
    }
    unsafe { core::slice::from_raw_parts_mut(meta as *mut u8, 4096).fill(0); }
    let files = (meta + OFF_FILES as u64) as *mut boruix::File;
    let ptrs = (meta + OFF_PTRS as u64) as *mut *mut boruix::File;

    for (i, (path, cmdline)) in specs.iter().enumerate() {
        let f = match vol.open_path(bs, dev, path) {
            Ok(f) => f,
            Err(s) => {
                serial::write(format_args!("[m8] module {} open failed err={:#x}\n", i, s));
                return Err(0xD2);
            }
        };
        let size = f.size as u64;
        let pages = ((size + 0xFFF) / 0x1000).max(1);
        let mut data: u64 = 0;
        let st2 = unsafe {
            (bs.allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::MEMORY_LOADER_DATA, pages as usize, &mut data)
        };
        if efi::is_error(st2) {
            return Err(0xD3);
        }
        let buf = unsafe { core::slice::from_raw_parts_mut(data as *mut u8, size as usize) };
        let n = match vol.read_file(dev, &f, buf) {
            Ok(n) => n,
            Err(s) => {
                serial::write(format_args!("[m8] module {} read failed err={:#x}\n", i, s));
                return Err(0xD4);
            }
        };
        if n as u64 != size {
            return Err(0xD5);
        }
        let mut sum: u32 = 0;
        for &b in buf.iter() {
            sum = sum.wrapping_add(b as u32);
        }

        let path_off = OFF_STR + i * 2 * MAX_STR;
        let cmd_off = path_off + MAX_STR;
        let path_phys = write_cstr(meta, path_off, path)?;
        let cmd_phys = write_cstr(meta, cmd_off, cmdline)?;

        let file = boruix::File {
            revision: 0,
            base: (data + boruix::HHDM_OFFSET) as *mut u8,
            length: size,
            path: (path_phys + boruix::HHDM_OFFSET) as *mut u8,
            cmdline: (cmd_phys + boruix::HHDM_OFFSET) as *mut u8,
            media_type: boruix::MEDIA_OPTICAL,
            unused: 0,
            tftp_ip: 0,
            tftp_port: 0,
            partition_index: 0,
            mbr_disk_id: 0,
            gpt_disk_uuid: [0; 16],
            gpt_part_uuid: [0; 16],
            part_uuid: [0; 16],
        };
        unsafe {
            core::ptr::write(files.add(i), file);
            core::ptr::write(
                ptrs.add(i),
                (files.add(i) as u64 + boruix::HHDM_OFFSET) as *mut boruix::File,
            );
        }
        serial::write(format_args!(
            "[m8] module {} path={} len={} sum16={:#06x}\n",
            i, path, size, sum as u16
        ));
    }

    Ok(LoadedModules { count: specs.len(), files, ptrs })
}

/// 把 `s` 以 NUL 结尾写进结构页，返回其**物理**地址。
fn write_cstr(page: u64, off: usize, s: &str) -> Result<u64, usize> {
    let b = s.as_bytes();
    if b.len() + 1 > MAX_STR {
        return Err(0xD6);
    }
    unsafe {
        let dst = (page + off as u64) as *mut u8;
        core::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
        core::ptr::write(dst.add(b.len()), 0);
    }
    Ok(page + off as u64)
}
