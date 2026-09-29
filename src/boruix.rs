//! BORUIX v1 交接协议——Limine 协议语义子集，响应结构字节级兼容 brxlimine-rs。
//!
//! 交付面（内核实际消费的 7 项）：
//!   BaseRevision、Hhdm、Memmap、Framebuffer、Rsdp、KernelFile、KernelAddress
//! 裁剪（内核各消费点均有 NULL 退化路径）：SMP（单核）、BootInfo、StackSize、
//! Terminal、Module、Smbios、EfiSystemTable、BootTime、Dtb。
//!
//! 交接方式与 Limine 相同：扫描内核映像内的请求标记（24 字节：COMMON_MAGIC
//! 2×u64 + 请求 id 2×u64 + revision + response 槽），按 id 匹配后原位填 response
//! 指针；BaseRevision 的 revision 槽原位写 0 表示支持。
//!
//! 结构布局事实源：kernel/vendor/brxlimine-rs/src/lib.rs（repr(C) 逐字段对齐）。

// ---------------------------------------------------------------- 协议常量

/// 请求标记 COMMON_MAGIC（brxlimine-rs lib.rs:195）。
pub const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// BaseRevision 标记 id（lib.rs:297）。
pub const BASE_REVISION_ID: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

/// 各请求 id（lib.rs 各 make_struct! 行）。
pub const HHDM_ID: [u64; 2] = [0x48dcf1cb8ad2b852, 0x63984e959a98244b];
pub const MEMMAP_ID: [u64; 2] = [0x67cf3d9d378a806f, 0xe304acdfc50c3c62];
pub const FRAMEBUFFER_ID: [u64; 2] = [0x9d5827dcd881dd75, 0xa3148604f6fab11b];
pub const RSDP_ID: [u64; 2] = [0xc5e77b6b397e7b43, 0x27637845accdcf3c];
pub const KERNEL_FILE_ID: [u64; 2] = [0xad97e90e83f1ed67, 0x31eb5d1c5ff23b69];
pub const KERNEL_ADDRESS_ID: [u64; 2] = [0x71ba76863cc55f63, 0xb2644a48c516a487];
/// 模块请求 id（brxlimine-rs lib.rs 689 make_struct!）。
pub const MODULE_ID: [u64; 2] = [0x3e7e279702be32af, 0xca1c4f3bd1280cee];

/// SMP 请求 id（brxlimine-rs lib.rs 584 make_struct!）。
pub const SMP_ID: [u64; 2] = [0x95a67b819a1b857e, 0xa0b61b723b6a73e0];

/// HHDM 偏移（Limine x86_64 语义）。
pub const HHDM_OFFSET: u64 = 0xFFFF_8000_0000_0000;

/// Memmap 类型（lib.rs MemoryMapEntryType）。
pub const MEMMAP_USABLE: u64 = 0;
/// bootloader 可回收区（内核快照页表根后可复用；liftoff 自占区用此类型）。
pub const MEMMAP_BOOTLOADER_RECLAIMABLE: u64 = 5;
pub const MEMMAP_RESERVED: u64 = 1;
pub const MEMMAP_ACPI_RECLAIMABLE: u64 = 2;
pub const MEMMAP_ACPI_NVS: u64 = 3;
pub const MEMMAP_KERNEL_AND_MODULES: u64 = 6;
pub const MEMMAP_FRAMEBUFFER: u64 = 7;

// ---------------------------------------------------------------- 响应结构

/// HhdmResponse（lib.rs 342）。
#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

/// MemmapEntry（lib.rs 614）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemmapEntry {
    pub base: u64,
    pub len: u64,
    pub typ: u64,
}

/// MemmapResponse（lib.rs 622）。
#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *mut MemmapEntry,
}

/// Framebuffer（lib.rs 355；内核 drivers.rs 消费 address/width/height/pitch/bpp/
/// memory_model 与 rgb mask）。
#[repr(C)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    pub reserved: [u8; 7],
    pub edid_size: u64,
    pub edid: *mut u8,
}

/// FramebufferResponse（lib.rs 382）。
#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *mut *mut Framebuffer,
}

/// RsdpResponse（lib.rs 695）。
#[repr(C)]
pub struct RsdpResponse {
    pub revision: u64,
    pub address: *mut u8,
}

/// File（lib.rs 256；内核 boot_source() 消费 media_type/partition_index/mbr_disk_id）。
#[repr(C)]
pub struct File {
    pub revision: u64,
    pub base: *mut u8,
    pub length: u64,
    pub path: *mut u8,
    pub cmdline: *mut u8,
    pub media_type: u32,
    pub unused: u32,
    pub tftp_ip: u32,
    pub tftp_port: u32,
    pub partition_index: u32,
    pub mbr_disk_id: u32,
    pub gpt_disk_uuid: [u8; 16],
    pub gpt_part_uuid: [u8; 16],
    pub part_uuid: [u8; 16],
}

/// KernelFileResponse（lib.rs 661）。
#[repr(C)]
pub struct KernelFileResponse {
    pub revision: u64,
    pub kernel_file: *mut File,
}

/// KernelAddressResponse（lib.rs 749）。
#[repr(C)]
pub struct KernelAddressResponse {
    pub revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

/// SmpInfo（brxlimine-rs lib.rs 531，32B）：每 CPU 一个，AP 停在
/// goto_address=0 的 HLT 轮询里，原子写非零即跳转（RDI=&SmpInfo，64KiB 栈）。
#[repr(C)]
pub struct SmpInfo {
    pub processor_id: u32,
    pub lapic_id: u32,
    pub reserved: u64,
    pub goto_address: u64,
    pub extra_argument: u64,
}

const _: () = assert!(core::mem::size_of::<SmpInfo>() == 32);

/// SmpResponse（brxlimine-rs lib.rs 550）。cpus 是指针数组（ArrayPtr，
/// memmap 同款语义——内核逐槽解引用）。
#[repr(C)]
pub struct SmpResponse {
    pub revision: u64,
    pub flags: u32,
    pub bsp_lapic_id: u32,
    pub cpu_count: u64,
    pub cpus: *mut *mut SmpInfo,
}

const _: () = assert!(core::mem::size_of::<SmpResponse>() == 32);

/// ModuleResponse（brxlimine-rs lib.rs 674）：模块即 File（同 KernelFile 的 File），
/// `modules` 是 module_count 个 **File 指针** 的数组（ArrayPtr 语义）。
#[repr(C)]
pub struct ModuleResponse {
    pub revision: u64,
    pub module_count: u64,
    pub modules: *mut *mut File,
}

const _: () = assert!(core::mem::size_of::<ModuleResponse>() == 24);

/// media_type：Limine 语义（内核 main.rs LIMINE_MEDIA_*）。
pub const MEDIA_OPTICAL: u32 = 1;

// ---------------------------------------------------------------- 交接数据集

/// 一次交接需要的全部响应区（单块连续分配，地址记入协议包）。
pub struct Handover {
    pub hhdm: HhdmResponse,
    pub memmap: MemmapResponse,
    pub fb: FramebufferResponse,
    pub fb_ptr: *mut Framebuffer,
    pub fb_struct: Framebuffer,
    pub rsdp: RsdpResponse,
    pub kfile: KernelFileResponse,
    pub file_struct: File,
    pub kaddr: KernelAddressResponse,
    /// 帧缓冲物理基址（loader 内部 memmap 剥离用；fb_struct.address 是
    /// HHDM 虚地址，协议语义不可逆推物理）。0 = 无 GOP。
    pub fb_phys: u64,
    pub smp: SmpResponse,
    pub smp_infos: *mut SmpInfo,
    pub smp_ptrs: *mut *mut SmpInfo,
    pub modules: ModuleResponse,
    /// 模块 File 结构数组（物理地址；load 时填充）。
    pub module_files: *mut File,
    /// 模块 File 指针数组（物理地址；内核解引用为 HHDM 虚地址）。
    pub module_ptrs: *mut *mut File,
}

impl Handover {
    /// 构造交接数据（从已取得的 UEFI 事实填充）。
    pub fn build(
        hhdm_off: u64,
        memmap_entries: *mut MemmapEntry,
        memmap_count: usize,
        fb: Framebuffer,
        rsdp: *mut u8,
        kernel_base: u64,
        kernel_vbase: u64,
        kernel_len: u64,
        fb_phys: u64,
    ) -> Handover {
        Handover {
            hhdm: HhdmResponse { revision: 0, offset: hhdm_off },
            memmap: MemmapResponse { revision: 0, entry_count: memmap_count as u64, entries: memmap_entries },
            fb: FramebufferResponse { revision: 0, framebuffer_count: 1, framebuffers: core::ptr::null_mut() },
            fb_ptr: core::ptr::null_mut(),
            fb_struct: fb,
            fb_phys,
            rsdp: RsdpResponse { revision: 0, address: rsdp },
            kfile: KernelFileResponse { revision: 0, kernel_file: core::ptr::null_mut() },
            file_struct: File {
                revision: 0,
                base: core::ptr::null_mut(),
                length: kernel_len,
                path: core::ptr::null_mut(),
                cmdline: core::ptr::null_mut(),
                media_type: MEDIA_OPTICAL, // liftoff liveCD 语义：ISO 启动
                unused: 0,
                tftp_ip: 0,
                tftp_port: 0,
                partition_index: 0,
                mbr_disk_id: 0,
                gpt_disk_uuid: [0; 16],
                gpt_part_uuid: [0; 16],
                part_uuid: [0; 16],
            },
            kaddr: KernelAddressResponse {
                revision: 0,
                physical_base: kernel_base,
                virtual_base: kernel_vbase,
            },
            smp: SmpResponse {
                revision: 0,
                flags: 0,
                bsp_lapic_id: 0,
                cpu_count: 0,
                cpus: core::ptr::null_mut(),
            },
            smp_infos: core::ptr::null_mut(),
            smp_ptrs: core::ptr::null_mut(),
            modules: ModuleResponse { revision: 0, module_count: 0, modules: core::ptr::null_mut() },
            module_files: core::ptr::null_mut(),
            module_ptrs: core::ptr::null_mut(),
        }
    }
}
// ---------------------------------------------------------------- 请求扫描

/// 扫描内核映像中的 Limine 请求标记并填充响应。
///
/// 返回处理到的请求数。未知请求（内核声明但 BORUIX v1 不支持）跳过——
/// 内核侧 get_response 得 NULL，走各自退化路径（与 Limine 缺席同语义）。
///
/// SAFETY：image_base..image_end 必须是已装载且可写的内核映像物理区间。
pub unsafe fn fill_requests(
    image_base: u64,
    image_size: u64,
    handover: &mut Handover,
) -> usize {
    unsafe {
    let image = core::slice::from_raw_parts_mut(
        image_base as *mut u8,
        image_size as usize,
    );
    let mut filled = 0usize;
    let mut off = 0usize;
    while off + 48 <= image.len() {
        // 请求标记：COMMON_MAGIC(16) + id(16) + revision(8) + response(8) = 48B
        let id0 = u64::from_le_bytes(image[off..off + 8].try_into().unwrap());
        let id1 = u64::from_le_bytes(image[off + 8..off + 16].try_into().unwrap());
        if id0 != COMMON_MAGIC[0] || id1 != COMMON_MAGIC[1] {
            off += 8; // 按 8 字节步进滑窗
            continue;
        }
        let rid0 = u64::from_le_bytes(image[off + 16..off + 24].try_into().unwrap());
        let rid1 = u64::from_le_bytes(image[off + 24..off + 32].try_into().unwrap());
        let resp_slot = image_base + off as u64 + 40;
        let slot = resp_slot as *mut u64;
        // id 匹配（前两个 u64 是 COMMON_MAGIC，请求独有 id 在后两个）
        if [rid0, rid1] == HHDM_ID {
            *slot = (&mut handover.hhdm as *mut HhdmResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == MEMMAP_ID {
            *slot = (&mut handover.memmap as *mut MemmapResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == FRAMEBUFFER_ID {
            // fb_ptr 指向 fb_ptr 槽（指向指针数组）——单帧缓冲时 Limine 兼容布局：
            // FramebufferResponse.framebuffers 是 *mut *mut Framebuffer，我们让
            // fb_ptr 槽存 &fb_struct。
            handover.fb_ptr = (&mut handover.fb_struct as *mut Framebuffer as u64 + HHDM_OFFSET) as *mut Framebuffer;
            handover.fb.framebuffers = (&mut handover.fb_ptr as *mut *mut Framebuffer as u64 + HHDM_OFFSET) as *mut *mut Framebuffer;
            *slot = (&mut handover.fb as *mut FramebufferResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == RSDP_ID {
            *slot = (&mut handover.rsdp as *mut RsdpResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == KERNEL_FILE_ID {
            handover.kfile.kernel_file = (&mut handover.file_struct as *mut File as u64 + HHDM_OFFSET) as *mut File;
            *slot = (&mut handover.kfile as *mut KernelFileResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == KERNEL_ADDRESS_ID {
            *slot = (&mut handover.kaddr as *mut KernelAddressResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == MODULE_ID {
            // 模块 File/指针数组由 modules::load_from_iso 在 EBS 前填好（仅当
            // 内核声明本请求时才会加载；BORUIX 不声明 → 零行为变化）。
            if !handover.module_ptrs.is_null() {
                handover.modules.modules = (handover.module_ptrs as u64 + HHDM_OFFSET) as *mut *mut File;
            }
            *slot = (&mut handover.modules as *mut ModuleResponse as u64) + HHDM_OFFSET;
            filled += 1;
        } else if [rid0, rid1] == SMP_ID {
            // cpus 指针数组 + SmpInfo 数组由 smp 模块在 kick 前填好。
            handover.smp.cpus = (handover.smp_ptrs as u64 + HHDM_OFFSET) as *mut *mut SmpInfo;
            *slot = (&mut handover.smp as *mut SmpResponse as u64) + HHDM_OFFSET;
            filled += 1;
        }
        off += 48; // 命中或未知请求标记都跳过整个标记体
    }
    filled
    }
}

/// 扫描内核映像是否声明了指定请求标记（COMMON_MAGIC + id 的 48B 标记）。
/// M8 用于判定是否要加载模块：内核不声明 → 不读、不分配、零行为变化。
///
/// SAFETY：image_base..image_base+image_size 必须是已装载的内核映像区间。
pub unsafe fn has_request(image_base: u64, image_size: u64, id: [u64; 2]) -> bool {
    let image = unsafe { core::slice::from_raw_parts(image_base as *const u8, image_size as usize) };
    let mut off = 0usize;
    while off + 32 <= image.len() {
        let m0 = u64::from_le_bytes(image[off..off + 8].try_into().unwrap());
        let m1 = u64::from_le_bytes(image[off + 8..off + 16].try_into().unwrap());
        if m0 == COMMON_MAGIC[0] && m1 == COMMON_MAGIC[1] {
            let i0 = u64::from_le_bytes(image[off + 16..off + 24].try_into().unwrap());
            let i1 = u64::from_le_bytes(image[off + 24..off + 32].try_into().unwrap());
            if i0 == id[0] && i1 == id[1] {
                return true;
            }
            off += 48;
            continue;
        }
        off += 8;
    }
    false
}
/// BaseRevision 特殊处理：id 前缀不同于请求标记（无 COMMON_MAGIC），
/// 结构是 id(16) + revision(8) = 24B；支持的 revision 原位写 0。
///
/// SAFETY：同 fill_requests。
pub unsafe fn fill_base_revision(
    image_base: u64,
    image_size: u64,
    max_revision: u64,
) -> bool {
    unsafe {
    let image = core::slice::from_raw_parts_mut(
        image_base as *mut u8,
        image_size as usize,
    );
    let mut off = 0usize;
    while off + 24 <= image.len() {
        let id0 = u64::from_le_bytes(image[off..off + 8].try_into().unwrap());
        let id1 = u64::from_le_bytes(image[off + 8..off + 16].try_into().unwrap());
        if id0 == BASE_REVISION_ID[0] && id1 == BASE_REVISION_ID[1] {
            let rev_slot = image_base + off as u64 + 16;
            let rev = u64::from_le_bytes(image[off + 16..off + 24].try_into().unwrap());
            if rev <= max_revision {
                (rev_slot as *mut u64).write_volatile(0); // 0 = 支持
            }
            return true;
        }
        off += 8;
    }
    false
    }
}