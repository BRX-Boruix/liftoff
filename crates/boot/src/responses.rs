//! 协议响应容器：把“内核声明的请求 → 我们准备好的响应结构”集中在一处。
//!
//! **关键约束（安全前提）**：一旦把某个响应的指针交给内核（写入请求的 `response` 字段），
//! 该结构**就不能再移动** —— 否则内核持有的是失效指针。因此本容器必须**只构造一次**、
//! 之后**不再搬动**（不要放进会被移动的临时变量、也不要按值返回它）。
//!
//! 边界：本模块只依赖 `limine` 的协议结构（纯数据）与 `core`；不含任何固件访问 ——
//! 具体数值由上层用 `firmware` 抽象取来后经 setter 填入。

use core::ffi::c_void;
use firmware::error::Error;
use firmware::graphics::FramebufferInfo;
use firmware::memory::{MemoryEntry, MemoryMapSource};
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
use limine::mp::{MP_REQUEST_ID, MpInfo, MpResponse};
use limine::rsdp::{RSDP_REQUEST_ID, RsdpResponse};

/// 零值 UUID（表示「未知」）。
const ZERO_UUID: limine::file::Uuid = limine::file::Uuid { a: 0, b: 0, c: 0, d: [0; 8] };

/// 内核在介质上的路径（NUL 结尾，供 `File::path` 使用）。
static KERNEL_PATH: &[u8] = b"/boot/kernel\0";

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
    /// 可执行文件的描述本体（`executable_file` 指向它，故必须留在容器内）。
    executable_file_data: File,
    executable_address: ExecutableAddressResponse,
    entry_point: EntryPointResponse,
    module: ModuleResponse,
    modules: [*mut File; MAX_MODULES],
    mp: MpResponse,
    /// BSP 的 `MpInfo`（`mp.cpus` 指向它的地址，故必须留在容器内）。
    mp_info: MpInfo,
    /// CPU 指针数组（`mp.cpus` 指向它）。
    mp_cpu_ptrs: [*mut MpInfo; 1],
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
            executable_file_data: File {
                revision: 0,
                address: core::ptr::null_mut(),
                size: 0,
                path: core::ptr::null_mut(),
                string: core::ptr::null_mut(),
                media_type: 0,
                unused: 0,
                tftp_ipv4: [0; 4],
                tftp_port: 0,
                partition_index: 0,
                mbr_disk_id: 0,
                gpt_disk_uuid: ZERO_UUID,
                gpt_part_uuid: ZERO_UUID,
                part_uuid: ZERO_UUID,
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
            mp_info: MpInfo {
                processor_id: 0,
                lapic_id: 0,
                reserved: 0,
                goto_address: None,
                extra_argument: 0,
            },
            mp_cpu_ptrs: [core::ptr::null_mut(); 1],
        }
    }

    /// 填充 SMP 响应：**至少**登记启动处理器（BSP）本身。
    ///
    /// `cpu_count = 0` 会让内核认为没有任何 CPU —— 真机实测：内核在每 CPU 初始化
    /// （`[cpu] ... enabled`）之后卡死在紧循环里（24 秒内全部寄存器逐位不变）。
    ///
    /// 我们**不启动**任何 AP（`goto_address = None`），也不声称支持 x2APIC。
    /// 回显 SMP 请求里的标志（`MP_REQUEST_X86_64_X2APIC` → 响应同位置位）。
    ///
    /// 内核若请求 x2APIC 而响应没有回显，它会去走 xAPIC（MMIO）路径 —— 那条路在
    /// 我们交付的状态下可能失败，进而每 CPU 初始化失败并 panic。
    pub fn set_smp_flags(&mut self, flags: u32) {
        self.mp.flags |= flags;
    }

    pub fn set_smp(&mut self, bsp_lapic_id: u32) {
        self.mp_info = MpInfo {
            processor_id: 0,
            lapic_id: bsp_lapic_id,
            reserved: 0,
            goto_address: None,
            extra_argument: 0,
        };
        let info: *mut MpInfo = &mut self.mp_info;
        self.mp_cpu_ptrs = [info];
        self.mp.revision = 0;
        self.mp.flags = 0;
        self.mp.bsp_lapic_id = bsp_lapic_id;
        self.mp.cpu_count = 1;
        self.mp.cpus = self.mp_cpu_ptrs.as_mut_ptr();
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

/// 用固件抽象的内存映射填充 `Responses`：把抽象类型**翻译成协议取值**后交给容器。
///
/// 取映射失败时**不写入任何条目**（宁可容器保持空，也不写半份数据）。
pub fn fill_memory_map<S: MemoryMapSource>(
    responses: &mut Responses,
    source: &mut S,
    buffer: &mut [MemoryEntry],
) -> Result<usize, Error> {
    let map = source.memory_map(buffer)?;
    let count = if map.len() > MAX_MEMMAP_ENTRIES { MAX_MEMMAP_ENTRIES } else { map.len() };
    let mut converted = [limine::memmap::MemmapEntry { base: 0, length: 0, kind: 0 }; MAX_MEMMAP_ENTRIES];
    for (index, entry) in map.iter().take(count).enumerate() {
        converted[index] = limine::memmap::MemmapEntry {
            base: entry.base.as_u64(),
            length: entry.length,
            kind: entry.kind.as_protocol() as u64,
        };
    }
    responses.set_memmap(&converted[..count]);
    Ok(count)
}

#[cfg(test)]
mod fill_from_firmware_tests {
    use super::{Responses, fill_memory_map};
    use arch::addr::PhysAddr;
    use firmware::error::Error;
    use firmware::memory::{MemoryEntry, MemoryKind, MemoryMap, MemoryMapSource};
    use limine::memmap::{RESERVED, USABLE};
    use std::vec::Vec;

    /// 假内存映射来源。
    struct FakeMap {
        entries: Vec<MemoryEntry>,
        fail: bool,
    }

    impl MemoryMapSource for FakeMap {
        fn memory_map<'b>(&mut self, buffer: &'b mut [MemoryEntry]) -> Result<MemoryMap<'b>, Error> {
            if self.fail {
                return Err(Error::Io);
            }
            if buffer.len() < self.entries.len() {
                return Err(Error::BufferTooSmall);
            }
            let count = self.entries.len();
            buffer[..count].copy_from_slice(&self.entries);
            Ok(MemoryMap::new(&buffer[..count]))
        }
    }

    fn entry(base: u64, length: u64, kind: MemoryKind) -> MemoryEntry {
        MemoryEntry { base: PhysAddr::new(base), length, kind }
    }

    #[test]
    fn the_firmware_map_is_translated_into_protocol_kinds() {
        let mut source = FakeMap {
            entries: std::vec![
                entry(0x1000, 0x2000, MemoryKind::Usable),
                entry(0x5000, 0x1000, MemoryKind::Reserved),
            ],
            fail: false,
        };
        let mut buffer = [entry(0, 0, MemoryKind::Reserved); 8];
        let mut responses = Responses::new();
        let count = fill_memory_map(&mut responses, &mut source, &mut buffer).expect("填充成功");
        assert_eq!(count, 2);
        let raw = responses.pointer_for(&limine::memmap::MEMMAP_REQUEST_ID).expect("有响应");
        // SAFETY: `raw` 指向 `responses` 内字段，类型为 `MemmapResponse`。
        let memmap: &limine::memmap::MemmapResponse =
            unsafe { &*(raw as *const limine::memmap::MemmapResponse) };
        assert_eq!(memmap.entry_count, 2);
        // SAFETY: `entries` 指向容器内指针数组，长度为 2。
        let first = unsafe { **memmap.entries };
        assert_eq!(first.base, 0x1000);
        assert_eq!(first.length, 0x2000);
        assert_eq!(first.kind, USABLE, "抽象类型必须翻译成协议取值");
        // SAFETY: 同上，第二项也在数组内。
        let second = unsafe { **(memmap.entries.add(1)) };
        assert_eq!(second.kind, RESERVED);
    }

    #[test]
    fn a_failing_source_leaves_the_container_untouched() {
        let mut source = FakeMap { entries: Vec::new(), fail: true };
        let mut buffer = [entry(0, 0, MemoryKind::Reserved); 8];
        let mut responses = Responses::new();
        assert!(fill_memory_map(&mut responses, &mut source, &mut buffer).is_err());
        let raw = responses.pointer_for(&limine::memmap::MEMMAP_REQUEST_ID).expect("有响应");
        // SAFETY: 同上。
        let memmap: &limine::memmap::MemmapResponse =
            unsafe { &*(raw as *const limine::memmap::MemmapResponse) };
        assert_eq!(memmap.entry_count, 0, "取映射失败时不得写入任何条目");
    }
}

/// 用固件报告的帧缓冲填充 `Responses`。
///
/// 没有 EDID、没有模式列表时**如实置空**（不编造）；信息无效则返回 `InvalidArgument`
/// 且**不写入任何条目**。
pub fn fill_framebuffer(responses: &mut Responses, info: &FramebufferInfo) -> Result<(), Error> {
    if !info.is_valid() {
        return Err(Error::InvalidArgument);
    }
    // **报 HHDM 地址，不是物理地址**（对照 brxLimine `limine.c:1487`：
    // `fbp[i].address = reported_addr(fbs[i].framebuffer_addr)`，而
    // `reported_addr(a) = a + direct_map_offset`）。
    //
    // 这不是风格问题：HHDM 地址在**内核的所有地址空间**里都映射；裸物理地址只在
    // 初始恒等映射里映射。真机实测的后果：内核终端在内核地址空间初始化成功
    // （`fb=0x80000000`），切到 PID 1 的地址空间后写同一地址即 #PF
    // （`CR2=0x803d46e8`、错误码 `0x2`）。
    let address = crate::entry::HHDM_OFFSET.wrapping_add(info.base.as_u64());
    let entry = limine::framebuffer::Framebuffer {
        address: address as *mut core::ffi::c_void,
        width: info.width as u64,
        height: info.height as u64,
        pitch: info.pitch as u64,
        bpp: info.format.bits_per_pixel,
        memory_model: limine::framebuffer::MEMORY_MODEL_RGB,
        red_mask_size: info.format.red_mask_size,
        red_mask_shift: info.format.red_shift,
        green_mask_size: info.format.green_mask_size,
        green_mask_shift: info.format.green_shift,
        blue_mask_size: info.format.blue_mask_size,
        blue_mask_shift: info.format.blue_shift,
        unused: [0; 7],
        edid_size: 0,
        edid: core::ptr::null_mut(),
        mode_count: 0,
        modes: core::ptr::null_mut(),
    };
    responses.set_framebuffer(&[entry]);
    Ok(())
}

#[cfg(test)]
mod fill_framebuffer_tests {
    use super::{Responses, fill_framebuffer};
    use arch::addr::PhysAddr;
    use firmware::error::Error;
    use firmware::graphics::{FramebufferInfo, PixelFormat};
    use limine::framebuffer::{FRAMEBUFFER_REQUEST_ID, FramebufferResponse, MEMORY_MODEL_RGB};

    fn info(base: u64, width: u32, height: u32, pitch: u32, bpp: u16) -> FramebufferInfo {
        FramebufferInfo {
            base: PhysAddr::new(base),
            width,
            height,
            pitch,
            format: PixelFormat {
                bits_per_pixel: bpp,
                red_shift: 16,
                green_shift: 8,
                blue_shift: 0,
                red_mask_size: 8,
                green_mask_size: 8,
                blue_mask_size: 8,
            },
        }
    }

    #[test]
    fn the_response_carries_the_firmware_values_verbatim() {
        let mut responses = Responses::new();
        let fb = info(0xfd00_0000, 1024, 768, 4096, 32);
        fill_framebuffer(&mut responses, &fb).expect("填充成功");
        let raw = responses.pointer_for(&FRAMEBUFFER_REQUEST_ID).expect("有响应");
        // SAFETY: `raw` 指向容器内字段，类型为 `FramebufferResponse`。
        let response: &FramebufferResponse = unsafe { &*(raw as *const FramebufferResponse) };
        assert_eq!(response.framebuffer_count, 1);
        // SAFETY: `framebuffers` 指向容器内指针数组，长度为 1。
        let entry = unsafe { **(response.framebuffers) };
        // **必须是 HHDM 地址**（对照 brxLimine `reported_addr`）：HHDM 在内核所有
        // 地址空间都映射，裸物理地址只在初始恒等映射里映射 —— 后者会在切到用户
        // 地址空间后 #PF（真机实测 CR2=0x803d46e8）。
        assert_eq!(
            entry.address as u64,
            crate::entry::HHDM_OFFSET + 0xfd00_0000,
            "帧缓冲地址必须是物理地址 + direct_map_offset"
        );
        assert_eq!(entry.width, 1024);
        assert_eq!(entry.height, 768);
        assert_eq!(entry.pitch, 4096);
        assert_eq!(entry.bpp, 32);
        assert_eq!(entry.memory_model, MEMORY_MODEL_RGB, "UEFI 像素格式映射到 RGB 模型");
        assert_eq!(entry.red_mask_shift, 16);
        assert_eq!(entry.red_mask_size, 8);
        assert_eq!(entry.green_mask_size, 8);
        assert_eq!(entry.blue_mask_size, 8);
        assert_eq!(entry.edid_size, 0, "没有 EDID 就如实置空，不编造");
        assert_eq!(entry.mode_count, 0, "没有模式列表就如实置空");
    }

    #[test]
    fn an_invalid_framebuffer_is_rejected() {
        let mut responses = Responses::new();
        let bad = info(0xfd00_0000, 0, 768, 4096, 32);
        assert_eq!(fill_framebuffer(&mut responses, &bad), Err(Error::InvalidArgument));
        let raw = responses.pointer_for(&FRAMEBUFFER_REQUEST_ID).expect("有响应");
        // SAFETY: 同上。
        let response: &FramebufferResponse = unsafe { &*(raw as *const FramebufferResponse) };
        assert_eq!(response.framebuffer_count, 0, "无效输入不得写入任何帧缓冲");
    }
}

#[cfg(test)]
mod executable_and_entry_tests {
    use super::Responses;
    use limine::entry_point::{ENTRY_POINT_REQUEST_ID, EntryPointResponse};
    use limine::executable_address::{EXECUTABLE_ADDRESS_REQUEST_ID, ExecutableAddressResponse};

    #[test]
    fn the_executable_address_response_carries_both_bases() {
        let mut responses = Responses::new();
        responses.set_executable_address(0x10_0000, 0xffff_ffff_8000_0000);
        let raw = responses
            .pointer_for(&EXECUTABLE_ADDRESS_REQUEST_ID)
            .expect("有响应");
        // SAFETY: `raw` 指向容器内字段，类型为 `ExecutableAddressResponse`。
        let response: &ExecutableAddressResponse =
            unsafe { &*(raw as *const ExecutableAddressResponse) };
        assert_eq!(response.physical_base, 0x10_0000);
        assert_eq!(response.virtual_base, 0xffff_ffff_8000_0000);
    }

    #[test]
    fn the_entry_point_response_is_a_zero_revision_marker() {
        let mut responses = Responses::new();
        let raw = responses.pointer_for(&ENTRY_POINT_REQUEST_ID).expect("有响应");
        // SAFETY: 同上；该响应只有 `revision` 一个字段。
        let response: &EntryPointResponse = unsafe { &*(raw as *const EntryPointResponse) };
        assert_eq!(response.revision, 0, "入口点响应只是一个 revision 标记，无其他载荷");
    }
}

/// 用装载结果填充「可执行文件」响应。
///
/// 未跟踪的信息（分区索引、磁盘标识、UUID）**一律置零表示未知**，不编造；
/// `media_type` 取 `MEDIA_TYPE_GENERIC`（未指明具体介质类型）；没有命令行则 `string` 置空。
pub fn fill_executable_file(
    responses: &mut Responses,
    address: u64,
    size: u64,
) -> Result<(), Error> {
    responses.executable_file_data = File {
        revision: 0,
        address: address as *mut core::ffi::c_void,
        size,
        path: KERNEL_PATH.as_ptr() as *mut core::ffi::c_char,
        string: core::ptr::null_mut(),
        media_type: limine::file::MEDIA_TYPE_GENERIC,
        unused: 0,
        tftp_ipv4: [0; 4],
        tftp_port: 0,
        partition_index: 0,
        mbr_disk_id: 0,
        gpt_disk_uuid: ZERO_UUID,
        gpt_part_uuid: ZERO_UUID,
        part_uuid: ZERO_UUID,
    };
    responses.executable_file.executable_file = &mut responses.executable_file_data;
    Ok(())
}

#[cfg(test)]
mod fill_executable_file_tests {
    use super::{Responses, fill_executable_file};
    use limine::executable_file::{EXECUTABLE_FILE_REQUEST_ID, ExecutableFileResponse};
    use limine::file::{File, MEDIA_TYPE_GENERIC};

    #[test]
    fn the_executable_file_response_describes_the_loaded_kernel() {
        let mut responses = Responses::new();
        fill_executable_file(&mut responses, 0x10_0000, 24_619_400).expect("填充成功");
        let raw = responses
            .pointer_for(&EXECUTABLE_FILE_REQUEST_ID)
            .expect("有响应");
        // SAFETY: `raw` 指向容器内字段，类型为 `ExecutableFileResponse`。
        let response: &ExecutableFileResponse =
            unsafe { &*(raw as *const ExecutableFileResponse) };
        assert!(!response.executable_file.is_null(), "必须给出可执行文件描述");
        // SAFETY: 指针指向容器内的 `File`。
        let file: &File = unsafe { &*response.executable_file };
        assert_eq!(file.address as u64, 0x10_0000, "地址是装载后的物理位置");
        assert_eq!(file.size, 24_619_400, "大小取自实测的内核长度");
        assert_eq!(file.media_type, MEDIA_TYPE_GENERIC, "未指明具体介质类型时不冒充");
        assert_eq!(file.partition_index, 0, "未跟踪分区索引就置零，不编造");
        assert_eq!(file.mbr_disk_id, 0);
        assert!(!file.path.is_null(), "路径必须给出");
        // SAFETY: 路径是静态 NUL 结尾字节串。
        let path = unsafe { core::ffi::CStr::from_ptr(file.path) };
        assert_eq!(path.to_bytes(), b"/boot/kernel");
        assert!(file.string.is_null(), "没有命令行就置空");
        assert_eq!(file.tftp_port, 0, "非网络引导");
    }
}
/// 把内核占用的物理区间在内存映射里标成 `KernelAndModules`（必要时**拆分**条目）。
///
/// # 为什么必须有这一步
///
/// 内核依据内存映射决定哪些内存可用。内核自己占的页是我们用固件 `AllocatePages`
/// 分配的，在响应里会显示成 `BootloaderReclaimable` —— 内核会把它并入空闲池，
/// 随后**踩掉自己的代码或数据**。对照 brxLimine：它用 `MEMMAP_KERNEL_AND_MODULES`
/// 装载内核与模块，并在 `base_revision` 相关规则里把该类型从空闲映射里排除。
///
/// `ranges` 是内核占用的**物理**区间 `(base, len)`。
///
/// 返回写入 `out` 的条目数；**缓冲不足时返回 `None`**（调用方保留原映射，绝不
/// 静默产出错误的映射）。
pub fn mark_kernel_memory(
    entries: &[limine::memmap::MemmapEntry],
    ranges: &[(u64, u64)],
    out: &mut [limine::memmap::MemmapEntry],
) -> Option<usize> {
    use firmware::memory::MemoryKind;
    // 一个条目被至多 `ranges.len()` 个区间切分，片段数上界 = 2*len + 1。
    const MAX_PIECES: usize = 2 * 8 + 1;
    let mut written = 0usize;
    for entry in entries {
        let mut pieces = [(0u64, 0u64, false); MAX_PIECES];
        let mut count = 0usize;
        pieces[count] = (entry.base, entry.length, false);
        count += 1;
        for &(range_base, range_len) in ranges.iter().take(8) {
            let range_end = range_base.checked_add(range_len)?;
            let mut next = [(0u64, 0u64, false); MAX_PIECES];
            let mut next_count = 0usize;
            for &(base, len, inside) in pieces.iter().take(count) {
                let end = base.checked_add(len)?;
                if end <= range_base || base >= range_end {
                    if next_count == MAX_PIECES {
                        return None;
                    }
                    next[next_count] = (base, len, inside);
                    next_count += 1;
                    continue;
                }
                if base < range_base {
                    if next_count == MAX_PIECES {
                        return None;
                    }
                    next[next_count] = (base, range_base - base, inside);
                    next_count += 1;
                }
                let low = if base > range_base { base } else { range_base };
                let high = if end < range_end { end } else { range_end };
                if next_count == MAX_PIECES {
                    return None;
                }
                next[next_count] = (low, high - low, true);
                next_count += 1;
                if end > range_end {
                    if next_count == MAX_PIECES {
                        return None;
                    }
                    next[next_count] = (range_end, end - range_end, inside);
                    next_count += 1;
                }
            }
            pieces = next;
            count = next_count;
        }
        for &(base, len, inside) in pieces.iter().take(count) {
            if len == 0 {
                continue;
            }
            if written == out.len() {
                return None;
            }
            out[written] = limine::memmap::MemmapEntry {
                base,
                length: len,
                kind: if inside {
                    MemoryKind::KernelAndModules.as_protocol() as u64
                } else {
                    entry.kind
                },
            };
            written += 1;
        }
    }
    Some(written)
}
