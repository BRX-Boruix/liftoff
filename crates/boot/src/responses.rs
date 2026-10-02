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

/// 最多登记的 CPU 数。
///
/// 取 256：远超任何现实机器的插槽数，而 `256 * size_of::<MpInfo>()` 只有 8 KiB，
/// 对引导器是零成本。**超出即拒绝登记**（不静默丢），见 `set_smp_cpus`。
pub const MAX_CPUS: usize = 256;

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
    /// 每个 CPU 的 `MpInfo`（`mp.cpus` 指向 `mp_cpu_ptrs`，后者指向这里，故必须留在容器内）。
    mp_infos: [MpInfo; MAX_CPUS],
    /// CPU 指针数组（`mp.cpus` 指向它）。
    mp_cpu_ptrs: [*mut MpInfo; MAX_CPUS],
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
            mp_infos: [MpInfo {
                processor_id: 0,
                lapic_id: 0,
                reserved: 0,
                goto_address: None,
                extra_argument: 0,
            }; MAX_CPUS],
            mp_cpu_ptrs: [core::ptr::null_mut(); MAX_CPUS],
        }
    }

    /// 填充 SMP 响应：登记启动处理器（BSP）本身。
    ///
    /// **只报 1 个 CPU 是诚实的，不是省略**（E1，第 256 轮实测）：我们确实只让 BSP
    /// 可用 —— 不启动任何 AP。真机 4 核实测内核看到 `total cpus=1`、向用户态报
    /// `cores: 1`，即**多核不被利用但也不会崩**。
    ///
    /// 反过来「枚举 MADT 里所有 CPU 但 `goto_address = None`」是**更糟**的做法：
    /// 内核会去启动它启动不了的 AP。所以 `cpu_count = 1` 是当前能力下唯一诚实的答复。
    ///
    /// 真正支持多核需要：AP 启动跳板、每 CPU 栈、以及让内核能写 `goto_address` ——
    /// 属独立工作项（对标 W-1/W-2），**是否投入由所有者决定**。
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

    /// 只登记 BSP（单核情形）—— 等价于「MADT 里只有 BSP、一个 AP 都没启动」。
    pub fn set_smp(&mut self, bsp_lapic_id: u32) {
        let bsp = utils::acpi::MadtCpu { processor_id: 0, apic_id: bsp_lapic_id, enabled: true };
        self.set_smp_cpus(core::slice::from_ref(&bsp), 0);
    }

    /// 登记 MADT 里的 CPU，并声明**实际启动了几个 AP**；返回登记到的 CPU 数。
    ///
    /// `cpu_count` = 1（BSP）+ 实际启动的 AP 数 —— **只报真正起来的 CPU**。
    /// 把没启动的 CPU 也报给内核是**更糟**的做法：内核会去用它启动不了的 AP
    /// （真机实测 `cpu_count = 0` 会让内核卡死在紧循环，故 BSP 必须计入）。
    ///
    /// 三条不变量：
    /// 1. 只登记固件声明为 `enabled` 的 CPU —— 对标 brxLimine，它只尝试启动 enabled 的 AP。
    /// 2. 超过 `MAX_CPUS` 的部分**拒绝登记**（不静默截断成"看起来成功"）。
    /// 3. `started_aps` **夹到**可用 AP 数以内 —— 调用方声称起了比存在的更多的核时，
    ///    不把 `cpu_count` 报成不可能的值。
    ///
    /// 约定：`cpus[0]` 是 BSP（MADT 的枚举顺序即此），空切片表示"没有可用 CPU"，
    /// 此时 `cpu_count = 0`（如实反映，而不是假报一个 BSP）。
    pub fn set_smp_cpus(&mut self, cpus: &[utils::acpi::MadtCpu], started_aps: usize) -> usize {
        let mut registered = 0usize;
        for cpu in cpus {
            if !cpu.enabled || registered >= MAX_CPUS {
                continue;
            }
            self.mp_infos[registered] = MpInfo {
                processor_id: cpu.processor_id,
                lapic_id: cpu.apic_id,
                reserved: 0,
                goto_address: None,
                extra_argument: 0,
            };
            self.mp_cpu_ptrs[registered] = &mut self.mp_infos[registered];
            registered += 1;
        }
        let aps = registered.saturating_sub(1);
        let started = started_aps.min(aps);
        self.mp.revision = 0;
        self.mp.bsp_lapic_id = if registered == 0 { 0 } else { self.mp_infos[0].lapic_id };
        self.mp.cpu_count = if registered == 0 { 0 } else { (1 + started) as u64 };
        self.mp.cpus = self.mp_cpu_ptrs.as_mut_ptr();
        registered
    }

    /// 只更新**实际启动的 AP 数**（即 `cpu_count`），**不动已登记的 `MpInfo`** ✓。
    ///
    /// **为什么不复用 `set_smp_cpus`**：那会**逐个重写** `MpInfo`（含 `goto_address`）✗ ——
    /// 而此时 AP **正在轮询 `goto_address`** ✓，重写是没必要的并发写 ✗；`reserved`（内核填的
    /// AP 栈）更不该被引导器碰 ✗。所以启动完成后只改计数 ✓。
    ///
    /// `registered` 是 `set_smp_cpus` 的返回值 —— 用它把 `started_aps` **夹**到可用 AP 数
    /// 以内 ✓（调用方声称起了比存在的更多的核时，不把 `cpu_count` 报成不可能的值 ✗）。
    pub fn set_started_aps(&mut self, registered: usize, started_aps: usize) {
        if registered == 0 {
            self.mp.cpu_count = 0;
            return;
        }
        let aps = registered.saturating_sub(1);
        let started = started_aps.min(aps);
        self.mp.cpu_count = (1 + started) as u64;
    }

    /// 当前报告给内核的 CPU 数。
    pub fn mp_cpu_count(&self) -> u64 {
        self.mp.cpu_count
    }

    /// 第 `index` 个 CPU 的描述；越界返回 `None`。
    pub fn mp_info_at(&self, index: usize) -> Option<&MpInfo> {
        self.mp_infos.get(index)
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
        // **revision = 1**：brxLimine 同值（limine.c:1468）。内核据此判断响应字段可用性。
        self.framebuffer.revision = 1;
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
        // **revision = 2**：brxLimine 同值（limine.c:1210）。当前内核未声明 modules
        // 请求，但一旦声明，这就是最容易悄悄错的字段。
        self.module.revision = 2;
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
/// 请求覆盖：内核声明什么，我们就必须应答什么。
///
/// 为什么值得一组专门的测试：这是目标项 ①「按内核声明的请求真正填充」的**可执行定义**。
/// 此前的证据只是「内核能启动」—— 那证明的是"至少用到的那些填对了"，不是"声明集被完整
/// 覆盖"。E5 排查时正是靠一次性的 Python 对齐才发现 7/7 全覆盖，但那个结论没有任何东西
/// 守着：将来内核多声明一个请求，我们会**静默不应答**，而内核可能只是降级而不是报错。
#[cfg(test)]
mod smp_tests {
    use super::Responses;
    use limine::mp::MpInfo;
    use utils::acpi::MadtCpu;

    fn cpu(processor_id: u32, apic_id: u32) -> MadtCpu {
        MadtCpu { processor_id, apic_id, enabled: true }
    }

    #[test]
    fn only_started_aps_are_reported_to_the_kernel() {
        // **这是安全属性，不是格式问题。** 把没启动的 CPU 也报给内核，内核会去用它
        // 启动不了的 AP —— 我自己在 `set_smp` 的注释里把那种做法记为「更糟」。
        // 所以 `cpu_count` 必须 = 1（BSP）+ **实际启动**的 AP 数。
        let mut r = Responses::new();
        let cpus = [cpu(0, 0), cpu(1, 1), cpu(2, 2), cpu(3, 3)];
        r.set_smp_cpus(&cpus, 0);
        assert_eq!(r.mp_cpu_count(), 1, "一个 AP 都没起来时只能报 BSP");
        r.set_smp_cpus(&cpus, 2);
        assert_eq!(r.mp_cpu_count(), 3, "起来 2 个 AP 就是 1+2");
    }

    #[test]
    fn each_reported_cpu_has_its_own_entry_with_the_right_apic_id() {
        let mut r = Responses::new();
        let cpus = [cpu(0, 7), cpu(1, 9)];
        r.set_smp_cpus(&cpus, 1);
        assert_eq!(r.mp_cpu_count(), 2);
        // 指针数组必须逐项指向**不同的** `MpInfo` —— 指向同一份，内核会把一个 CPU 看成两个。
        let a = r.mp_info_at(0).expect("第 0 项");
        let b = r.mp_info_at(1).expect("第 1 项");
        assert_ne!(a as *const MpInfo, b as *const MpInfo, "每项必须是独立的 MpInfo");
        assert_eq!(a.lapic_id, 7);
        assert_eq!(b.lapic_id, 9);
        assert_eq!(a.processor_id, 0);
        assert_eq!(b.processor_id, 1);
        assert!(a.goto_address.is_none(), "goto_address 由内核填，引导器不得预设");
    }

    #[test]
    fn the_chain_from_rsdp_to_registration_composes() {
        // **把 S1 与 S2 接起来。** RSDP → XSDT → 按签名找到 MADT → 解析出 CPU 列表 → 登记。
        // 每一段都单独测过，但"合起来能不能用"没人测过 —— 而接线处正是最容易出错的地方
        // （段与段各自正确、拼起来却对不上，这类问题在真机上表现为静默复位）。
        use utils::acpi::{MADT_HEADER_LEN, MADT_SIGNATURE, RSDP_SIGNATURE, SdtKind, cpus, find_table, parse_rsdp};

        const MADT_ADDR: u64 = 0x2000;

        // ① MADT：两项 xAPIC（处理器 0/1，APIC ID 0/1，均可用）。
        //    缓冲按**项数**算，不手写大小 —— 上一轮我就是手写大小而越界。
        const ENTRY_LEN: usize = 8;
        let mut madt = std::vec![0u8; MADT_HEADER_LEN + 2 * ENTRY_LEN];
        madt[0..4].copy_from_slice(MADT_SIGNATURE);
        for (index, apic_id) in [0u8, 1u8].into_iter().enumerate() {
            let at = MADT_HEADER_LEN + index * ENTRY_LEN;
            madt[at] = 0; // type 0 = xAPIC
            madt[at + 1] = ENTRY_LEN as u8;
            madt[at + 2] = apic_id; // processor id
            madt[at + 3] = apic_id; // apic id
            madt[at + 4..at + 8].copy_from_slice(&1u32.to_le_bytes()); // enabled
        }
        let madt_len = madt.len() as u32;
        madt[4..8].copy_from_slice(&madt_len.to_le_bytes());

        // ② XSDT：一项，指向 MADT。
        let mut xsdt = std::vec![0u8; 36 + 8];
        xsdt[0..4].copy_from_slice(b"XSDT");
        xsdt[36..44].copy_from_slice(&MADT_ADDR.to_le_bytes());
        let xsdt_len = xsdt.len() as u32;
        xsdt[4..8].copy_from_slice(&xsdt_len.to_le_bytes());

        // ③ RSDP：修订 2 → 用 XSDT。
        let mut rsdp = std::vec![0u8; 36];
        rsdp[0..8].copy_from_slice(RSDP_SIGNATURE);
        rsdp[15] = 2;
        rsdp[24..32].copy_from_slice(&0x1000u64.to_le_bytes()); // XSDT 的假地址

        // ④ 走完整条链。
        let root = parse_rsdp(&rsdp).expect("RSDP 应能解析");
        assert_eq!(root.kind, SdtKind::Xsdt, "修订 2 必须选 XSDT");

        let mut reader = |address: u64, out: &mut [u8; 4]| {
            if address == MADT_ADDR {
                out.copy_from_slice(MADT_SIGNATURE);
                true
            } else {
                false
            }
        };
        let found = find_table(&xsdt, root.kind, MADT_SIGNATURE, &mut reader)
            .expect("查找不应报错")
            .expect("必须找到 MADT");
        assert_eq!(found, MADT_ADDR);

        let mut list = [MadtCpu { processor_id: 0, apic_id: 0, enabled: false }; 4];
        let total = cpus(&madt, &mut list).expect("MADT 应能解析");
        assert_eq!(total, 2, "MADT 里声明了两颗 CPU");

        // ⑤ 登记：**一个 AP 都没启动**，所以仍然只报 BSP。
        let mut r = Responses::new();
        assert_eq!(r.set_smp_cpus(&list[..total], 0), 2, "两颗都登记上了");
        assert_eq!(r.mp_cpu_count(), 1, "没启动 AP 时只能报 BSP —— 这是安全属性");
        assert_eq!(r.mp_info_at(0).expect("第 0 项").lapic_id, 0);
        assert_eq!(r.mp_info_at(1).expect("第 1 项").lapic_id, 1);
    }

    #[test]
    fn claiming_more_started_aps_than_exist_is_rejected() {
        // 声称起了 9 个而表里只有 2 个 —— 不得把 `cpu_count` 报成 10。
        let mut r = Responses::new();
        let cpus = [cpu(0, 0), cpu(1, 1)];
        r.set_smp_cpus(&cpus, 9);
        assert_eq!(r.mp_cpu_count(), 2, "不得超过表里声明的 CPU 数");
    }
}

#[cfg(test)]
mod request_coverage_tests {
    use super::Responses;
    use limine::base::COMMON_MAGIC;
    use limine::scan::KNOWN_REQUESTS;

    /// ① 协议 crate 里定义的**每一个**请求，`Responses` 都必须有应答。
    ///
    /// 这条断言的是「我们声称实现的协议面」与「实际能应答的集合」一致 ——
    /// `KNOWN_REQUESTS` 是协议侧的单点定义（S15），不在这里重抄一份。
    #[test]
    fn every_known_request_has_a_response() {
        let mut responses = Responses::new();
        for (id, _size) in KNOWN_REQUESTS {
            assert!(
                responses.pointer_for(id).is_some(),
                "协议定义了请求 {:#x?}，但 Responses 没有对应应答",
                id,
            );
        }
    }

    /// ② 真实内核**声明**的每一个请求，都必须被应答。
    ///
    /// 按 `COMMON_MAGIC` 直接扫描，不依赖 START/END 标记 —— 内核可以不带标记而仍然
    /// 声明请求（实测它正是如此：7 个 `COMMON_MAGIC`）。
    #[test]
    fn every_request_the_kernel_declares_is_answered() {
        let Some(image) = crate::test_support::real_kernel() else {
            // 不静默跳过：产物不在时必须看得见。
            std::eprintln!("跳过：真实 ISO 不存在（无法验证内核声明集）");
            return;
        };
        let magic = [COMMON_MAGIC[0], COMMON_MAGIC[1]];
        let mut responses = Responses::new();
        let mut declared = 0usize;
        let mut at = 0usize;
        while at + 32 <= image.len() {
            let window = &image[at..at + 32];
            let mut words = [0u64; 4];
            for (index, slot) in words.iter_mut().enumerate() {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(&window[index * 8..index * 8 + 8]);
                *slot = u64::from_le_bytes(raw);
            }
            if words[0] == magic[0] && words[1] == magic[1] {
                declared += 1;
                let id = words;
                assert!(
                    responses.pointer_for(&id).is_some(),
                    "内核声明了请求 {:016x?}，但我们没有应答（会静默漏填）",
                    id,
                );
                at += 32;
                continue;
            }
            at += 8;
        }
        assert_eq!(declared, 7, "实测内核声明 7 个请求；数量变了说明内核换了声明集");
    }

    /// ③ 负对照：未知 ID 必须返回 `None`。
    ///
    /// 没有这条，上面两条断言就可能是「永远通过」的空断言 —— 若 `pointer_for` 对任何
    /// 输入都返回 `Some`，覆盖测试会毫无意义地全绿。
    #[test]
    fn an_unknown_request_has_no_response() {
        let mut responses = Responses::new();
        let bogus = [0u64, 0, 0xdead_beef_dead_beef, 0xfeed_face_feed_face];
        assert!(responses.pointer_for(&bogus).is_none(), "未知 ID 不应有应答");
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
#[cfg(test)]
mod set_modules_tests {
    use super::{Responses, MAX_MODULES};
    use limine::file::File;
    use limine::module::{MODULE_REQUEST_ID, ModuleResponse};
    use std::vec::Vec;

    /// 当前内核**没有**声明 modules 请求，但协议上它是 Limine 的一部分；保留
    /// `set_modules` 是为协议完整性。这个测试钉住两点：revision 与数量如实回报
    /// —— 一旦将来内核声明了它，这两条就是最容易悄悄错的。
    #[test]
    fn set_modules_reports_revision_two_and_honest_count() {
        let mut responses = Responses::new();
        // SAFETY: File 全由整数与裸指针组成（无 niched 类型），全零是合法位型；
        // 测试不解引用它们，只比较指针值与数量。
        let mut files: [File; 3] = [unsafe { core::mem::zeroed() }; 3];
        let pointers: [*mut File; 3] = [
            &mut files[0] as *mut File,
            &mut files[1] as *mut File,
            &mut files[2] as *mut File,
        ];
        responses.set_modules(&pointers);
        let raw = responses.pointer_for(&MODULE_REQUEST_ID).expect("有响应");
        // SAFETY: raw 指向容器内字段，类型为 ModuleResponse。
        let module: &ModuleResponse = unsafe { &*(raw as *const ModuleResponse) };
        // **revision = 2**：brxLimine 同值（limine.c:1210）。
        assert_eq!(module.revision, 2, "模块响应 revision 必须与 brxLimine 一致");
        assert_eq!(module.module_count, 3, "数量必须如实回报");
        for index in 0..3 {
            // SAFETY: entries[index] 指向容器内的 File。
            let file = unsafe { *module.modules.add(index) };
            assert_eq!(file as *mut File, pointers[index], "模块指针顺序必须保持");
        }
    }

    #[test]
    fn set_modules_over_capacity_is_reported_honestly() {
        let mut responses = Responses::new();
        // SAFETY: 同上 —— 全零 File 只用于占位比较，从不解引用。
        let mut files: [File; MAX_MODULES + 1] = [unsafe { core::mem::zeroed() }; MAX_MODULES + 1];
        let pointers: Vec<*mut File> = files.iter_mut().map(|f| f as *mut File).collect();
        responses.set_modules(&pointers);
        let raw = responses.pointer_for(&MODULE_REQUEST_ID).expect("有响应");
        // SAFETY: raw 指向容器内字段。
        let module: &ModuleResponse = unsafe { &*(raw as *const ModuleResponse) };
        assert_eq!(
            module.module_count,
            MAX_MODULES as u64,
            "超容量时只登记前 MAX_MODULES 个，且数量如实",
        );
    }
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
        // **revision 必须是 1**：brxLimine 设 framebuffer_response->revision = 1
        // （limine.c:1468）。revision 是响应的版本契约，内核据此判断哪些字段可用；
        // 报 0 而实际填了 revision-1 才有的内容，等于对内核撒谎。
        assert_eq!(response.revision, 1, "帧缓冲响应 revision 必须与 brxLimine 一致");
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
        // **HHDM 地址**（对照 brxLimine `limine.c:424/427`：`ret.address = reported_addr(file->fd)`）。
        address: crate::entry::HHDM_OFFSET.wrapping_add(address) as *mut core::ffi::c_void,
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
        // **必须是 HHDM 地址**（对照 brxLimine `limine.c:424/427`：`ret.address = reported_addr(file->fd)`）。
        // 裸物理地址只在初始恒等映射里存在，切到用户地址空间后内核读它就会 #PF。
        assert_eq!(
            file.address as u64,
            crate::entry::HHDM_OFFSET + 0x10_0000,
            "地址必须是物理地址 + direct_map_offset"
        );
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
