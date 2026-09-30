//! 协议响应容器：把“内核声明的请求 → 我们准备好的响应结构”集中在一处。
//!
//! **关键约束（安全前提）**：一旦把某个响应的指针交给内核（写入请求的 `response` 字段），
//! 该结构**就不能再移动** —— 否则内核持有的是失效指针。因此本容器必须**只构造一次**、
//! 之后**不再搬动**（不要放进会被移动的临时变量、也不要按值返回它）。
//!
//! 边界：本模块只依赖 `limine` 的协议结构（纯数据）与 `core`；不含任何固件访问 ——
//! 具体数值由上层用 `firmware` 抽象取来后经 setter 填入。

use core::ffi::c_void;
use limine::base::{HHDM_REQUEST_ID, HhdmResponse};
use limine::bootloader_info::{BOOTLOADER_INFO_REQUEST_ID, BootloaderInfoResponse};
use limine::entry_point::{ENTRY_POINT_REQUEST_ID, EntryPointResponse};
use limine::executable_address::{EXECUTABLE_ADDRESS_REQUEST_ID, ExecutableAddressResponse};
use limine::executable_file::{EXECUTABLE_FILE_REQUEST_ID, ExecutableFileResponse};
use limine::file::File;
use limine::firmware_type::{FIRMWARE_TYPE_REQUEST_ID, FirmwareTypeResponse};
use limine::framebuffer::{FRAMEBUFFER_REQUEST_ID, Framebuffer, FramebufferResponse};
use limine::memmap::{MEMMAP_REQUEST_ID, MemmapEntry, MemmapResponse};
use limine::module::{MODULE_REQUEST_ID, ModuleResponse};
use limine::mp::{MP_REQUEST_ID, MpResponse};
use limine::rsdp::{RSDP_REQUEST_ID, RsdpResponse};

/// 内存映射最多登记的条目数（写入前检查容量）。
pub const MAX_MEMMAP_ENTRIES: usize = 32;
/// 最多登记的帧缓冲数。
pub const MAX_FRAMEBUFFERS: usize = 4;
/// 最多登记的模块数。
pub const MAX_MODULES: usize = 16;

/// 协议响应容器。
pub struct Responses {
    hhdm: HhdmResponse,
    memmap: MemmapResponse,
    memmap_entries: [MemmapEntry; MAX_MEMMAP_ENTRIES],
    memmap_pointers: [*mut MemmapEntry; MAX_MEMMAP_ENTRIES],
    rsdp: RsdpResponse,
    framebuffer: FramebufferResponse,
    framebuffers: [Framebuffer; MAX_FRAMEBUFFERS],
    framebuffer_pointers: [*mut Framebuffer; MAX_FRAMEBUFFERS],
    bootloader_info: BootloaderInfoResponse,
    firmware_type: FirmwareTypeResponse,
    executable_file: ExecutableFileResponse,
    executable_address: ExecutableAddressResponse,
    entry_point: EntryPointResponse,
    module: ModuleResponse,
    modules: [*mut File; MAX_MODULES],
    mp: MpResponse,
}

impl Responses {
    /// 构造一个全空的容器。
    pub const fn new() -> Self {
        Self {
            hhdm: HhdmResponse { revision: 0, offset: 0 },
            memmap: MemmapResponse { revision: 0, entry_count: 0, entries: core::ptr::null_mut() },
            memmap_entries: [MemmapEntry { base: 0, length: 0, kind: 0 }; MAX_MEMMAP_ENTRIES],
            memmap_pointers: [core::ptr::null_mut(); MAX_MEMMAP_ENTRIES],
            rsdp: RsdpResponse { revision: 0, address: core::ptr::null_mut() },
            framebuffer: FramebufferResponse {
                revision: 0,
                framebuffer_count: 0,
                framebuffers: core::ptr::null_mut(),
            },
            framebuffers: [Framebuffer::EMPTY; MAX_FRAMEBUFFERS],
            framebuffer_pointers: [core::ptr::null_mut(); MAX_FRAMEBUFFERS],
            bootloader_info: BootloaderInfoResponse {
                revision: 0,
                name: core::ptr::null_mut(),
                version: core::ptr::null_mut(),
            },
            firmware_type: FirmwareTypeResponse { revision: 0, firmware_type: 0 },
            executable_file: ExecutableFileResponse {
                revision: 0,
                executable_file: core::ptr::null_mut(),
            },
            executable_address: ExecutableAddressResponse {
                revision: 0,
                physical_base: 0,
                virtual_base: 0,
            },
            entry_point: EntryPointResponse { revision: 0 },
            module: ModuleResponse { revision: 0, module_count: 0, modules: core::ptr::null_mut() },
            modules: [core::ptr::null_mut(); MAX_MODULES],
            mp: MpResponse {
                revision: 0,
                flags: 0,
                bsp_lapic_id: 0,
                cpu_count: 0,
                cpus: core::ptr::null_mut(),
            },
        }
    }

    /// 设置 HHDM 偏移。
    pub fn set_hhdm_offset(&mut self, offset: u64) {
        self.hhdm.offset = offset;
    }

    /// 设置固件类型。
    pub fn set_firmware_type(&mut self, kind: u64) {
        self.firmware_type.firmware_type = kind;
    }

    /// 登记内存映射条目（超出容量则只登记前 `MAX_MEMMAP_ENTRIES` 条，并把 `entry_count`
    /// 设为**实际登记数** —— 不谎报数量）。
    pub fn set_memmap(&mut self, entries: &[MemmapEntry]) {
        let count = if entries.len() > MAX_MEMMAP_ENTRIES { MAX_MEMMAP_ENTRIES } else { entries.len() };
        let mut index = 0;
        while index < count {
            self.memmap_entries[index] = entries[index];
            self.memmap_pointers[index] = &mut self.memmap_entries[index];
            index += 1;
        }
        self.memmap.entry_count = count as u64;
        self.memmap.entries = self.memmap_pointers.as_mut_ptr();
    }

    /// 设置 RSDP 地址。
    pub fn set_rsdp(&mut self, address: *mut c_void) {
        self.rsdp.address = address;
    }

    /// 设置引导器名称与版本（`&'static` 字节串，须以 NUL 结尾）。
    pub fn set_bootloader_info(&mut self, name: *mut core::ffi::c_char, version: *mut core::ffi::c_char) {
        self.bootloader_info.name = name;
        self.bootloader_info.version = version;
    }

    /// 设置可执行文件地址。
    pub fn set_executable_address(&mut self, physical_base: u64, virtual_base: u64) {
        self.executable_address.physical_base = physical_base;
        self.executable_address.virtual_base = virtual_base;
    }

    /// 设置内核入口。
    pub fn set_entry_point(&mut self, entry: Option<limine::entry_point::EntryPoint>) {
        self.entry_point.revision = 0;
        let _ = entry;
    }

    /// 登记帧缓冲（超容量则只登记前 `MAX_FRAMEBUFFERS` 个，并把 `framebuffer_count`
    /// 设为**实际登记数**，不谎报数量）。
    pub fn set_framebuffer(&mut self, framebuffers: &[Framebuffer]) {
        let count = if framebuffers.len() > MAX_FRAMEBUFFERS {
            MAX_FRAMEBUFFERS
        } else {
            framebuffers.len()
        };
        let mut index = 0;
        while index < count {
            self.framebuffers[index] = framebuffers[index];
            self.framebuffer_pointers[index] = &mut self.framebuffers[index];
            index += 1;
        }
        self.framebuffer.framebuffer_count = count as u64;
        self.framebuffer.framebuffers = self.framebuffer_pointers.as_mut_ptr();
    }

    /// 登记模块（超容量则只登记前 `MAX_MODULES` 个，并如实回报数量）。
    pub fn set_modules(&mut self, modules: &[*mut File]) {
        let count = if modules.len() > MAX_MODULES { MAX_MODULES } else { modules.len() };
        let mut index = 0;
        while index < count {
            self.modules[index] = modules[index];
            index += 1;
        }
        self.module.module_count = count as u64;
        self.module.modules = self.modules.as_mut_ptr();
    }

    /// 按请求 ID 取出对应响应的指针；未知请求返回 `None`。
    pub fn pointer_for(&mut self, id: &[u64; 4]) -> Option<*mut c_void> {
        let target: *mut c_void = if id == &HHDM_REQUEST_ID {
            &mut self.hhdm as *mut HhdmResponse as *mut c_void
        } else if id == &MEMMAP_REQUEST_ID {
            &mut self.memmap as *mut MemmapResponse as *mut c_void
        } else if id == &RSDP_REQUEST_ID {
            &mut self.rsdp as *mut RsdpResponse as *mut c_void
        } else if id == &FRAMEBUFFER_REQUEST_ID {
            &mut self.framebuffer as *mut FramebufferResponse as *mut c_void
        } else if id == &BOOTLOADER_INFO_REQUEST_ID {
            &mut self.bootloader_info as *mut BootloaderInfoResponse as *mut c_void
        } else if id == &FIRMWARE_TYPE_REQUEST_ID {
            &mut self.firmware_type as *mut FirmwareTypeResponse as *mut c_void
        } else if id == &EXECUTABLE_FILE_REQUEST_ID {
            &mut self.executable_file as *mut ExecutableFileResponse as *mut c_void
        } else if id == &EXECUTABLE_ADDRESS_REQUEST_ID {
            &mut self.executable_address as *mut ExecutableAddressResponse as *mut c_void
        } else if id == &ENTRY_POINT_REQUEST_ID {
            &mut self.entry_point as *mut EntryPointResponse as *mut c_void
        } else if id == &MODULE_REQUEST_ID {
            &mut self.module as *mut ModuleResponse as *mut c_void
        } else if id == &MP_REQUEST_ID {
            &mut self.mp as *mut MpResponse as *mut c_void
        } else {
            return None;
        };
        Some(target)
    }
}
#[cfg(test)]
mod tests {
    use super::Responses;
    use limine::base::{HHDM_REQUEST_ID, HhdmResponse};
    use limine::firmware_type::FIRMWARE_TYPE_REQUEST_ID;
    use limine::memmap::MEMMAP_REQUEST_ID;
    use limine::rsdp::RSDP_REQUEST_ID;

    #[test]
    fn each_known_request_gets_its_own_non_null_pointer() {
        let mut responses = Responses::new();
        responses.set_hhdm_offset(0xffff_8000_0000_0000);
        responses.set_firmware_type(limine::firmware_type::EFI64);
        let hhdm = responses.pointer_for(&HHDM_REQUEST_ID).expect("HHDM 有响应");
        let firmware = responses.pointer_for(&FIRMWARE_TYPE_REQUEST_ID).expect("固件类型有响应");
        let memmap = responses.pointer_for(&MEMMAP_REQUEST_ID).expect("内存映射有响应");
        let rsdp = responses.pointer_for(&RSDP_REQUEST_ID).expect("RSDP 有响应");
        assert!(!hhdm.is_null());
        assert!(!firmware.is_null());
        assert!(!memmap.is_null());
        assert!(!rsdp.is_null());
        assert_ne!(hhdm, firmware, "不同请求的响应必须是不同对象");
        assert_ne!(memmap, rsdp);
    }

    #[test]
    fn an_unknown_request_has_no_response() {
        let mut responses = Responses::new();
        assert!(responses.pointer_for(&[1, 2, 3, 4]).is_none());
    }

    #[test]
    fn the_hhdm_response_carries_the_offset_we_set() {
        let mut responses = Responses::new();
        responses.set_hhdm_offset(0xffff_8000_0000_0000);
        let raw = responses.pointer_for(&HHDM_REQUEST_ID).expect("HHDM 有响应");
        // SAFETY: `raw` 由 `pointer_for` 给出，指向本函数栈上的 `responses` 内字段；
        // 在 `responses` 存活期间有效，且类型正是 `HhdmResponse`。
        let hhdm: &HhdmResponse = unsafe { &*(raw as *const HhdmResponse) };
        assert_eq!(hhdm.offset, 0xffff_8000_0000_0000);
        assert_eq!(hhdm.revision, 0);
    }

    #[test]
    fn the_memmap_response_reports_what_we_put_in_it() {
        let mut responses = Responses::new();
        // 传真实条目（而不是个数）：这样才能验证“报告的就是放进去的”。
        responses.set_memmap(&[
            limine::memmap::MemmapEntry { base: 0x1000, length: 0x2000, kind: limine::memmap::USABLE },
            limine::memmap::MemmapEntry { base: 0x5000, length: 0x1000, kind: limine::memmap::RESERVED },
        ]);
        let raw = responses.pointer_for(&MEMMAP_REQUEST_ID).expect("内存映射有响应");
        // SAFETY: 同上；`raw` 指向 `responses` 内字段，类型为 `MemmapResponse`。
        let memmap: &limine::memmap::MemmapResponse =
            unsafe { &*(raw as *const limine::memmap::MemmapResponse) };
        assert_eq!(memmap.entry_count, 2);
        assert!(!memmap.entries.is_null(), "条目数组指针必须已设置");
        // SAFETY: `entries` 指向本容器内的指针数组（长度 2），元素指向容器内条目。
        let first = unsafe { **memmap.entries };
        assert_eq!(first.base, 0x1000);
        assert_eq!(first.kind, limine::memmap::USABLE);
    }
}