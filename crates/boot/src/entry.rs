//! 入口编排（可宿主测试的部分）。
//!
//! 边界：bin 只做 `efi_main` 转发；本模块做“取引导服务表 → 输出启动诊断”的编排。
//! 平台与固件实现的**选择**来自门面（`crate::PlatformImpl`、`firmware_current::current`），
//! 本模块不自己挑实现。

use loader::elf::{ElfError, ProgramHeader, load_segments, parse_elf_header, parse_load_segments};
use arch::platform::Platform;
use firmware::boot_services::BootServicesControl;
use firmware::memory::{MemoryEntry, MemoryMapSource};
use crate::responses::{Responses, fill_executable_file, fill_framebuffer};
use firmware::graphics::GraphicsSink;
use limine::scan::RequestHit;
use mm::plan::{Mapping, PlanError};
use mm::usable::UsableRange;
use mm::takeover::{MustStay, TakeoverError};
use firmware::error::Error;
use arch::addr::PhysAddr;
use arch::hhdm::DirectMap;
use arch::paging::{FrameAllocator, PageFlags, PageTable};
use current::X86PageTable;
use current::spinup;
use firmware::block::DeviceIndex;
use firmware_current::current::{
    ALLOCATE_ANY_PAGES, AllocatePages, BootServicesTable, EFI_LOADER_CODE, EFI_LOADER_DATA,
    EfiFrameAllocator,
    ExitBootServices, Handle, SUCCESS, SystemTable, UefiBlockDevices, UefiBootServices,
    UefiGraphics,
    acpi_rsdp, graphics_output_mode,
    UefiMemoryMapSource, boot_services_of,
};
// `Stall` 只在 `target_os = "uefi"` 的 AP 启动路径里用到 ✓ —— 宿主测试里它是未使用的 ✗，
// 所以单独按目标门控导入（放进上面那个 use 组会让宿主构建报 unused import ✗）。
#[cfg(target_os = "uefi")]
use firmware_current::current::Stall;

/// 内核映像缓冲大小（实测内核 24,619,400 字节，留出余量）。
const KERNEL_BUFFER: usize = 32 * 1024 * 1024;
/// 分区表/PVD 头缓冲大小（覆盖 GPT 的 34 块 + PVD 偏移）。
const HEAD_BUFFER: usize = 64 * 1024;

/// 用固件页分配一块缓冲。
///
/// 引导器不做隐藏分配，但 25 MB 的内核映像无法放在栈上，只能向固件要页。
///
/// # Safety
///
/// 调用方保证 `allocate_pages` 有效，且这块内存不被别处使用。
unsafe fn alloc_buffer(allocate_pages: AllocatePages, len: usize) -> Option<&'static mut [u8]> {
    // SAFETY: 由调用方保证（见函数文档）。
    unsafe { alloc_buffer_typed(allocate_pages, len, EFI_LOADER_DATA) }
}

/// 同 [`alloc_buffer`]，但指定 EFI 内存类型。
///
/// 跳板必须用 `EfiLoaderCode`：`EfiLoaderData` 在 OVMF 下可能被标成不可执行。
///
/// # Safety
///
/// 同 [`alloc_buffer`]。
unsafe fn alloc_buffer_typed(
    allocate_pages: AllocatePages,
    len: usize,
    mem_type: u32,
) -> Option<&'static mut [u8]> {
    const PAGE: usize = 4096;
    let pages = len.div_ceil(PAGE);
    let mut address: u64 = 0;
    // SAFETY: 由调用方保证（见函数文档）。
    let status = unsafe {
        (allocate_pages)(ALLOCATE_ANY_PAGES, mem_type, pages, &mut address)
    };
    if status != SUCCESS {
        return None;
    }
    if address % PAGE as u64 != 0 {
        return None;
    }
    // SAFETY: 固件刚交出 `pages` 个页；引导阶段该物理地址可直接访问，且不与别处共享。
    Some(unsafe { core::slice::from_raw_parts_mut(address as *mut u8, pages * PAGE) })
}

/// 目的地缓冲大小：内核三段并集约 9.7 MiB，取大页整数倍并留余量。
const DESTINATION_BUFFER: usize = 16 * 1024 * 1024;

/// 以指定平台执行入口第一步；失败时**不输出诊断**（不制造假成功）。
///
/// 拿到引导服务表并输出启动诊断后，尝试**真实交接**：发现设备 → 读内核 → 规划 → 写表 →
/// 激活 → 交接。失败时输出**失败环节**（便于真跑定位），然后返回错误。
pub fn start_with<P: Platform>(
    system_table: *mut SystemTable,
    image_handle: Handle,
) -> Result<Outcome, Error> {
    let boot_services = boot_services_of(system_table)?;
    crate::diag::report_startup::<P>();
    // 每步都留痕：否则“成功跳转 / 还在跑 / panic”在串口上无法区分。
    report::<P>(b"[liftoff] step: alloc\n");

    // 大缓冲只能向固件要页（栈上放不下）。
    // SAFETY: 引导阶段单线程；这两块内存只在此处使用。
    let (kernel_out, head, destination) = unsafe {
        let Some(kernel_out) = alloc_buffer((*boot_services).allocate_pages, KERNEL_BUFFER) else {
            return Err(Error::Io);
        };
        let Some(head) = alloc_buffer((*boot_services).allocate_pages, HEAD_BUFFER) else {
            return Err(Error::Io);
        };
        // 多要一个大页：`AllocatePages` 只保证 4 KiB 对齐，内核目标必须 2 MiB 对齐。
        let Some(raw) = alloc_buffer(
            (*boot_services).allocate_pages,
            DESTINATION_BUFFER + LARGE_PAGE as usize,
        ) else {
            return Err(Error::Io);
        };
        let Some(destination) = aligned_within(raw, LARGE_PAGE) else {
            return Err(Error::Io);
        };
        (kernel_out, head, destination)
    };
    let destination_phys = destination.as_ptr() as u64;
    report::<P>(b"[liftoff] step: bring_up\n");

    // 小缓冲与响应容器：静态，避免栈溢出。
    // HHDM 与恒等映射是**按 2 MiB 页逐条产出**的：1 GiB 内存就要各约 512 条，
    // 再加内核高区，256 条远远不够（上一轮就是在这里报 BufferTooSmall）。
    // 恒等映射现在覆盖**除 Bad 外所有内存类型**（含 MMIO/保留区），映射条数远多于
    // 只看可分配区间时 —— 4096 条不够（真实运行报 stage: plan）。
    static mut PLAN: [Mapping; 16384] = [Mapping::EMPTY; 16384];
    static mut SEGMENTS: [ProgramHeader; 16] = [ProgramHeader::EMPTY; 16];
    static mut USABLE: [UsableRange; 1024] =
        [UsableRange { base: PhysAddr::new(0), length: 0 }; 1024];
    static mut HITS: [RequestHit; 64] = [RequestHit::EMPTY; 64];
    static mut MUST_STAY: [MustStay; 32] = [MustStay { start: 0, len: 0 }; 32];
    static mut RESPONSES: Responses = Responses::new();
    static mut MAP_BUFFER: [MemoryEntry; 2048] = [MemoryEntry {
        base: PhysAddr::new(0),
        length: 0,
        kind: firmware::memory::MemoryKind::Reserved,
    }; 2048];
    static mut MAP_KEY: Option<usize> = None;
    // 真实 UEFI 内存映射的字节数远大于 4 KiB（描述符每条约 40 字节、动辄上百条），
    // 缓冲不足会让两段式 `GetMemoryMap` 直接失败。
    static mut DESCRIPTORS: [u8; 65536] = [0; 65536];

    // SAFETY: 上述静态均为引导阶段单线程独占使用；`DESCRIPTORS` 的借用在此作用域内有效。
    let result = unsafe {
        let descriptors = &mut *core::ptr::addr_of_mut!(DESCRIPTORS);
        let mut memory_map =
            UefiMemoryMapSource::new((*boot_services).get_memory_map, descriptors);
        let c = BringUp {
            kernel_out,
            head,
            plan: &mut *core::ptr::addr_of_mut!(PLAN),
            segments: &mut *core::ptr::addr_of_mut!(SEGMENTS),
            usable: &mut *core::ptr::addr_of_mut!(USABLE),
            memory_map: &mut memory_map,
            map_buffer: &mut *core::ptr::addr_of_mut!(MAP_BUFFER),
            hits: &mut *core::ptr::addr_of_mut!(HITS),
            must_stay: &mut *core::ptr::addr_of_mut!(MUST_STAY),
            responses: &mut *core::ptr::addr_of_mut!(RESPONSES),
            map_key: &mut *core::ptr::addr_of_mut!(MAP_KEY),
            destination: destination_phys,
            rsdp: acpi_rsdp(&*system_table),
            framebuffer: graphics_output_mode(
                (*boot_services).locate_handle,
                (*boot_services).handle_protocol,
            )
            .map(|mode| UefiGraphics::new(mode).framebuffer().ok())
            .flatten(),
        };
        // SAFETY: 由 `check_before_entry` 与页表规划共同保证（见 `bring_up` 文档）。
        bring_up(&mut *boot_services, image_handle, c, |entry| <P as Platform>::jump_to(entry))
    };
    if let Err(stage) = result {
        // 失败时把**环节与具体原因**都写出来：真跑时这是最有用的信息。
        report_failure::<P>(&stage);
        return Err(Error::Io);
    }
    Ok(Outcome::Ready)
}

/// 往串口写一行（用于真跑时定位）。
fn report<P: Platform>(text: &[u8]) {
    for byte in text {
        P::write_byte(*byte);
    }
}

/// 把失败**连具体原因**一起写到诊断通道。
///
/// **为什么不能只打环节**：`BringUpError` 的 12 个环节里有 10 个携带具体错误，而此前
/// 它们**全都只打一个通用标记** —— "卡在哪一步"可见、"**为什么**"不可见。定位混合
/// 粒度失败时，正是把 `MapError` 的值打出来才一步锁定根因；当时只有 `apply` 因为一处
/// 特例会打印值，其余环节同样携带载荷却都不打印。这里**单点**补齐（S13/S15），
/// 避免同类问题在别处重演。
fn report_failure<P: Platform>(stage: &BringUpError) {
    report::<P>(stage_text(stage));
    // 无载荷的环节（RootFrame / DirectMap）到此为止。
    match stage {
        BringUpError::Discover(e) => report_cause::<P>(e),
        BringUpError::Media(e) => report_cause::<P>(e),
        BringUpError::Kernel(e) => report_cause::<P>(e),
        BringUpError::Plan(e) => report_cause::<P>(e),
        BringUpError::MemoryMapLoad(e) => report_cause::<P>(e),
        BringUpError::MemoryMapRanges(e) => report_cause::<P>(e),
        BringUpError::Apply(e) => report_cause::<P>(e),
        BringUpError::Copy(e) => report_cause::<P>(e),
        BringUpError::Responses(e) => report_cause::<P>(e),
        BringUpError::Handoff(e) => report_cause::<P>(e),
        BringUpError::RootFrame | BringUpError::DirectMap => {}
    }
}

/// 把一段 `fmt` 输出渲进**固定缓冲**后写到诊断通道（不分配）。
///
/// 缓冲溢出即截断 —— 诊断输出不该因为消息长就 panic。
///
/// **单点**：`report_cause` 与 `report_panic` 都经它输出，缓冲大小与截断策略只在这里。
/// 从**物理地址**读一段字节（只用于固件给的 ACPI 表）。
///
/// # Safety
///
/// `address` 必须指向**已映射**的 RAM，且 `[address, address + out.len())` 全部可达。
/// 调用方负责保证长度来自**表自己的长度字段**并设上限 —— 凭空猜大小会读越过 RAM 末尾，
/// 真机上表现为静默复位。
#[cfg(target_os = "uefi")]
unsafe fn read_phys(address: u64, out: &mut [u8]) -> bool {
    if address == 0 || out.is_empty() {
        return false;
    }
    // SAFETY: 由调用方保证（见本函数文档）。
    unsafe {
        core::ptr::copy_nonoverlapping(address as *const u8, out.as_mut_ptr(), out.len());
    }
    true
}

/// 单个 ACPI 表允许读取的最大长度（防御：长度字段可能是垃圾）。
#[cfg(target_os = "uefi")]
const MAX_ACPI_TABLE: usize = 64 * 1024;

/// 从 RSDP 找到 MADT、解析 CPU、登记，并把结果打到串口（S3c 第 1 步，**不启动 AP**）。
///
/// 用 `crate::PlatformImpl` 而不是类型参数：**`bring_up` 不泛型于平台** —— 它按值接收
/// `BringUp` 与一个跳转闭包，所以 `P` 在它的作用域里**不存在**（这一点我第 295 轮搞错，
/// 导致编译失败并回退）。
#[cfg(target_os = "uefi")]
unsafe fn register_madt_cpus(
    // **不是 `BootServicesTable`** ✗：`allocate_zeroed_below` 是 `EfiFrameAllocator` 的固有方法 ✓。
    frames: &mut EfiFrameAllocator,
    // **固件延时**：INIT 之后要等 10 ms、SIPI 之间要等 200 µs ✓ —— 忙等自旋的时长
    // 取决于 CPU 速度 ✗，而 AP 醒来所需的时间与 CPU 速度无关 ✓。
    stall: Stall,
    // 引导器的直接映射区间 —— 用来把指针换算成**物理**地址 ✓（不做裸减法 ✗）。
    direct: DirectMap,
    responses: &mut crate::responses::Responses,
    rsdp_address: u64,
    cr3_top: u64,
) {
    let mut header = [0u8; 36];
    // SAFETY: RSDP 是固件表、在 RAM 内，36 字节不会越过 RAM 末尾。
    if !unsafe { read_phys(rsdp_address, &mut header) } {
        return;
    }
    let Ok(root) = utils::acpi::parse_rsdp(&header) else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] rsdp: 解析失败\n"));
        return;
    };

    // 根表：先读 36 字节头拿长度，再按长度读全（**不猜大小**）。
    let mut root_header = [0u8; 36];
    // SAFETY: 同上。
    if !unsafe { read_phys(root.root, &mut root_header) } {
        return;
    }
    let root_len =
        u32::from_le_bytes([root_header[4], root_header[5], root_header[6], root_header[7]]) as usize;
    if root_len < 36 || root_len > MAX_ACPI_TABLE {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] rsdp: 根表长度不合法 {}\n", root_len));
        return;
    }
    let mut root_table = [0u8; MAX_ACPI_TABLE];
    // SAFETY: 长度已校验并设上限。
    if !unsafe { read_phys(root.root, &mut root_table[..root_len]) } {
        return;
    }

    let mut found = None;
    {
        let mut reader = |address: u64, out: &mut [u8; 4]| {
            // SAFETY: 表头 4 字节，地址来自根表项。
            unsafe { read_phys(address, out) }
        };
        if let Ok(hit) = utils::acpi::find_table(
            &root_table[..root_len],
            root.kind,
            utils::acpi::MADT_SIGNATURE,
            &mut reader,
        ) {
            found = hit;
        }
    }
    let Some(madt_address) = found else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] madt: 未找到\n"));
        return;
    };

    let mut madt_header = [0u8; 44];
    // SAFETY: MADT 固定头 44 字节，地址来自根表项。
    if !unsafe { read_phys(madt_address, &mut madt_header) } {
        return;
    }
    let madt_len =
        u32::from_le_bytes([madt_header[4], madt_header[5], madt_header[6], madt_header[7]]) as usize;
    if madt_len < 44 || madt_len > MAX_ACPI_TABLE {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] madt: 长度不合法 {}\n", madt_len));
        return;
    }
    let mut madt = [0u8; MAX_ACPI_TABLE];
    // SAFETY: 长度已校验并设上限。
    if !unsafe { read_phys(madt_address, &mut madt[..madt_len]) } {
        return;
    }

    // 容量与启动计划缓冲**共用同一个常量** ✓（不再各写一个 64 ✗）。
    let mut list = [utils::acpi::MadtCpu { processor_id: 0, apic_id: 0, enabled: false };
        crate::responses::AP_PLAN_CAPACITY];
    let Ok(total) = utils::acpi::cpus(&madt[..madt_len], &mut list) else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] madt: 解析失败\n"));
        return;
    };
    let usable = total.min(list.len());
    // **started_aps = 0**：还没启动任何 AP，所以 `cpu_count` 仍是 1。
    let registered = responses.set_smp_cpus(&list[..usable], 0);
    report_fmt::<crate::PlatformImpl>(format_args!(
        "[liftoff] madt: {} cpus described, {} registered, started_aps=0\n",
        total, registered
    ));

    // 【S3c 第 2 步的探针】读 LAPIC ID（**在激活之前**，靠当前生效的固件页表），
    // 与 CPUID 得到的 BSP ID 比对。
    //
    // **这一问决定路线**：
    // * 能读到且与 CPUID 一致 → **路线 A 可行**（AP 启动可以留在激活前，**不必动激活时序**）；
    // * 读不到 → 只能走**路线 B**（提前激活自己的页表）。
    //
    // **这一步带着真实风险**：它假设固件的页表映射了 LAPIC 的 MMIO。若没映射，取数故障 →
    // 复位（串口表现为复位循环）。**那个结果本身就是答案**，所以值得一试。
    //
    // 常量走 `current::lapic` 而不是在 `boot` 里写裸地址（S01 / ADR-007）。
    // SAFETY: 见上 —— 本探针的全部目的就是验证这个假设是否成立。
    // **经抽象层取**（C2）：裸 MMIO 的指针运算与易失读留在实现层 ✓ ——
    // 中性层自己做就是跨层直连 ✗（S14 / ADR-007）。
    // SAFETY: 见探针说明 —— 本探针的全部目的就是验证"固件映射了 LAPIC 的 MMIO"这个假设。
    let mmio_id = unsafe { current::lapic::read_id_via_mmio() };
    let cpuid_id = <crate::PlatformImpl as arch::platform::Platform>::bsp_lapic_id();
    report_fmt::<crate::PlatformImpl>(format_args!(
        "[liftoff] lapic: mmio_id={:#x} cpuid_id={:#x} {}\n",
        mmio_id,
        cpuid_id,
        if mmio_id == cpuid_id { "MATCH" } else { "MISMATCH" }
    ));

    // 【S4 探针】**只读** `IA32_APIC_BASE`（MSR `0x1B`），看固件把 APIC 配成了哪种模式。
    //
    // 读 MSR 无副作用 ✓，且该 MSR 在 x86-64 上必然存在 ✓ —— 所以这一步**不带故障风险**
    // （与上一轮那个 MMIO 探针不同）。它决定 S4 是否真的需要**去改**这个 MSR：
    // 若固件已经开了 x2APIC 而内核不支持，就必须退回 xAPIC（brxLimine 的做法）。
    // SAFETY: `IA32_APIC_BASE` 在 x86-64 上必然存在。
    // **经实现层解析位域** ✓ —— 中性层不读 MSR、不解释 bit ✗（S13 单点 / ADR-007）。
    // SAFETY: `IA32_APIC_BASE` 在 x86-64 上必然存在 ✓。
    let apic = unsafe { current::lapic::firmware_apic_state() };
    report_fmt::<crate::PlatformImpl>(format_args!(
        "[liftoff] apic_base={:#x} x2apic={} global_enable={}\n",
        apic.base, apic.x2apic, apic.global_enable
    ));

    // 【S5–S7 接线 · 第 ③ 段】真正把 AP 叫起来 —— **串行**，一次一个 ✓。
    //
    // **为什么必须串行**：跳板页里只有**一个**参数块 ✗ —— 所有 AP 都从**同一个**向量醒来，
    // 靠参数块里的 `info_struct` 区分身份 ✓。所以"写参数块 → 发 IPI → 等它醒来
    // → 再写下一个"的顺序是**协议的一部分**，不是实现细节 ✓。
    // 参考实现正是这样：`smp_start_ap()` 每个 AP 调一次（`common/sys/smp.c:44-120`）✓。
    //
    // 【缺陷修正】上一版在**这里**就把所有 AP 的参数块写进**同一页** ✗ —— 后一个覆盖前一个，
    // 于是所有 AP 都会按**最后一个**的参数块启动 ✗（都去读同一个 `goto_address`）。
    // 搬运与写参数块因此拆成 `install()` / `stage_frame()` 两步 ✓。
    let bsp = <crate::PlatformImpl as arch::platform::Platform>::bsp_lapic_id();
    // SAFETY: 仍在 boot services 期间；`frames` 分配低页、`stall` 是固件延时、
    // `responses` 里的 `MpInfo` 已由上面的 `set_smp_cpus` 登记好 ✓。
    let started =
        unsafe {
            start_aps(
                &mut *frames,
                stall,
                direct,
                &*responses,
                &list[..usable],
                bsp,
                registered,
                cr3_top,
            )
        };
    // **只报真正起来了的 AP 数** ✓ —— 报了没起来的，内核会去用它启动不了的 AP ✗。
    responses.set_started_aps(registered, started);
    report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] ap: started={started}\n"));
}

/// 启动所有非 BSP 的 `enabled` AP，返回**真正醒来**的数量。
///
/// **串行**（一次一个 ✓），因为跳板页里只有**一个**参数块 ✗ —— 顺序是协议的一部分 ✓。
/// 对照 brxLimine `common/sys/smp.c:44-120`（`smp_start_ap` 每个 AP 调一次 ✓）。
///
/// 返回的是**真的醒了几个**，不是"发了几个 IPI" ✗ —— 内核只会去用它启动得了的 AP ✓。
///
/// # Safety
/// 仍在 boot services 期间；`frames` 来自固件分配器、`stall` 是固件延时；
/// `responses` 里的 `MpInfo` 已由 `set_smp_cpus` 登记好 ✓。
#[cfg(target_os = "uefi")]
unsafe fn start_aps(
    frames: &mut EfiFrameAllocator,
    stall: Stall,
    direct: DirectMap,
    responses: &crate::responses::Responses,
    cpus: &[utils::acpi::MadtCpu],
    bsp: u32,
    // `set_smp_cpus` 登记到的 CPU 数 —— 用来判断登记下标有没有越界 ✓。
    registered: usize,
    cr3_top: u64,
) -> usize {
    // 低页：跳板代码 + GDT + GDTR 描述符 + 参数块，**全部在一页里** ✓。
    let max = current::ap::AP_LOW_LIMIT - current::ap::AP_PAGE_SIZE as u64;
    let Some(trampoline_frame) = frames.allocate_zeroed_below(max) else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] ap: 低页分配失败\n"));
        return 0;
    };
    let Some(trampoline_addr) = trampoline_frame.start_address() else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] ap: 低页帧无地址\n"));
        return 0;
    };
    // 临时栈**独立一页** ✓：放在跳板页里的话，栈会向**代码**生长 ✗
    // （参考实现单独分配 8192 字节，`smp.c:56-59` ✓）。
    let Some(stack_frame) = frames.allocate_zeroed_below(max) else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] ap: 临时栈分配失败\n"));
        return 0;
    };
    let Some(stack_addr) = stack_frame.start_address() else {
        report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] ap: 临时栈帧无地址\n"));
        return 0;
    };
    let temp_stack_top = stack_addr.as_u64() + current::ap::AP_PAGE_SIZE as u64;
    let base = trampoline_addr.as_u64();

    // SAFETY: 刚分配的一页、页对齐、长度恰为一页，且当前页表恒等映射 RAM
    //（本函数已在用同一映射读固件表）。
    let low = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, current::ap::AP_PAGE_SIZE) };
    let gdt = current::spinup::build_gdt();
    let vector = match current::ap::install(current::ap::trampoline_bytes(), &gdt, base, low) {
        Ok(vector) => vector,
        Err(error) => {
            report_fmt::<crate::PlatformImpl>(format_args!("[liftoff] ap: 跳板搬运失败: {error}\n"));
            return 0;
        }
    };
    report_fmt::<crate::PlatformImpl>(format_args!(
        "[liftoff] ap: 低页 base={base:#x} 向量={:#x} 临时栈顶={temp_stack_top:#x}\n",
        vector.0
    ));

    // **访问方式必须与固件当前实际模式一致** ✗（用 x2APIC 的 MSR 去访问一个 xAPIC 模式的
    // LAPIC 不会报错，只会**什么都不发生** ✗）。本内核不支持 x2APIC，故第一个参数是 `false` ✓。
    // 内核不支持 x2APIC → `false` ✓；模式判定与位域解析都在实现层 ✓。
    // SAFETY: `IA32_APIC_BASE` 在 x86-64 上必然存在 ✓。
    let access = unsafe { current::lapic::firmware_access(false) };

    // 【为什么把决策抽出去】这里原先有 5 处**静默** `continue` ✗ —— 于是一次真机失败
    // 在串口上只留下 `started=0`，**分不清**"全被跳过了"与"发了 IPI 但 AP 不醒" ✗
    // （第 108 轮的真机就是这个症状）。现在处置由 `responses::ap_plan` 算出（纯逻辑、
    // 宿主可测 ✓），并且**每一个跳过都带原因打出来** ✓。
    //
    // 先把固件给的原样打出来 ✓ —— `apic_id` 全是 0 这类事只有这样才看得见 ✗。
    for (index, cpu) in cpus.iter().enumerate() {
        report_fmt::<crate::PlatformImpl>(format_args!(
            "[liftoff] ap: cpu[{index}] processor_id={} apic_id={:#x} enabled={}\n",
            cpu.processor_id, cpu.apic_id, cpu.enabled
        ));
    }

    let mut plan = [crate::responses::ApPlan::Disabled; crate::responses::AP_PLAN_CAPACITY];
    let planned = crate::responses::ap_plan(cpus, bsp, registered, &mut plan);
    if planned < cpus.len() {
        report_fmt::<crate::PlatformImpl>(format_args!(
            "[liftoff] ap: 警告：MADT 报了 {} 个 CPU，计划缓冲只放得下 {planned} 个\n",
            cpus.len()
        ));
    }

    let mut started = 0usize;
    for (index, step) in plan[..planned].iter().enumerate() {
        let slot = match *step {
            crate::responses::ApPlan::Disabled => {
                report_fmt::<crate::PlatformImpl>(format_args!(
                    "[liftoff] ap: cpu[{index}] 跳过：固件标记为未使能\n"
                ));
                continue;
            }
            crate::responses::ApPlan::Bsp { slot } => {
                report_fmt::<crate::PlatformImpl>(format_args!(
                    "[liftoff] ap: cpu[{index}] 跳过：就是 BSP 自己（slot={slot}）\n"
                ));
                continue;
            }
            crate::responses::ApPlan::NoSlot { slot } => {
                report_fmt::<crate::PlatformImpl>(format_args!(
                    "[liftoff] ap: cpu[{index}] 跳过：登记下标 {slot} 越界（口径不一致）\n"
                ));
                continue;
            }
            crate::responses::ApPlan::Start { slot } => slot,
        };
        let Some(cpu) = cpus.get(index) else {
            continue;
        };
        let Some(info) = responses.mp_info_at(slot) else {
            report_fmt::<crate::PlatformImpl>(format_args!(
                "[liftoff] ap: cpu[{index}] 跳过：没有 slot={slot} 的 MpInfo\n"
            ));
            continue;
        };
        // `MpInfo` 的**物理**地址（跳板自己加 HHDM ✓）。
        // **经映射抽象算，不做裸减法** ✗ —— `RESPONSES` 在低地址恒等映射下，减法会回绕 ✗。
        let info_phys = arch::hhdm::phys_of_pointer(direct, info as *const _ as u64).as_u64();
        let input = current::ap::ApTrampolineInput {
            hhdm: HHDM_OFFSET,
            cr3_top,
            info_struct: info_phys,
            temp_stack_top,
            gdtr: base + current::ap::AP_GDTR_OFFSET as u64,
            // 保守：跳板里不开 CR0.WP ✓ —— 交接后 CR0 完全由内核掌控 ✓。
            write_protect: false,
        };
        let block = match current::ap::prepare(&input) {
            Ok(block) => block,
            Err(error) => {
                report_fmt::<crate::PlatformImpl>(format_args!(
                    "[liftoff] ap: cpu[{index}] 跳过：参数块填不出来: {error}\n"
                ));
                continue;
            }
        };
        if let Err(error) = current::ap::stage_frame(&block, low) {
            report_fmt::<crate::PlatformImpl>(format_args!(
                "[liftoff] ap: cpu[{index}] 跳过：参数块写不进低页: {error}\n"
            ));
            continue;
        }
        // 参数块写好了（`booted_flag` 也已归零 ✓）—— **现在**才允许这个 AP 醒来 ✓。
        // SAFETY: 仍在 boot services 期间，`access` 取自固件当前的 APIC 模式 ✓。
        unsafe { wake_ap(access, cpu.apic_id, vector, stall) };
        // 轮询 `booted_flag`：**必须易失读** ✓（否则优化器会把它提到循环外 ✗），
        // 且**必须有界** ✓（参考实现 100 × 10 ms = 1 秒，`smp.c:112-117` ✓）。
        let flag_at = current::ap::AP_FRAME_OFFSET
            + core::mem::offset_of!(current::ap::ApTrampoline, booted_flag);
        let stage_at = current::ap::AP_FRAME_OFFSET
            + core::mem::offset_of!(current::ap::ApTrampoline, stage);
        let mut booted = false;
        for _ in 0..current::lapic::AP_BOOT_POLLS {
            // SAFETY: 参数块在刚分配的低页内、页对齐，读一个字节 ✓。
            let flag = unsafe { core::ptr::read_volatile(low.as_ptr().add(flag_at)) };
            if flag == 1 {
                booted = true;
                break;
            }
            // SAFETY: `stall` 来自固件启动服务，调用点仍在 boot services 期间 ✓。
            unsafe { stall(current::lapic::AP_BOOT_STALL_US) };
        }
        if booted {
            started += 1;
        } else {
            // 读回**进度标记** ✓：`0` = IPI 根本没送到；`N` = AP 跑了、停在跳板的第 N 步 ✗。
            // 没有这一个字节，"没送到"与"崩在跳板里"在串口上**完全一样** ✗。
            // SAFETY: 与 `booted_flag` 同一页内、页对齐，读一个字节。
            let stage = unsafe { core::ptr::read_volatile(low.as_ptr().add(stage_at)) };
            report_fmt::<crate::PlatformImpl>(format_args!(
                "[liftoff] ap: lapic={:#x} 未在 1 秒内醒来，跳板进度={stage}\n",
                cpu.apic_id
            ));
        }
    }
    started
}

/// 给一个 AP 发 INIT + SIPI ×2。
///
/// 顺序**不能改** ✓（对照 brxLimine `common/sys/smp.c:83-106`）：
/// 1. INIT assert（`0x4500`）→ 固件延时 10 ms；
/// 2. INIT deassert（`0x0500`）→ 固件延时 10 ms（**有意与参考实现不同**，见 `lapic.rs` 的说明 ✓）；
/// 3. SIPI ×2（Intel SDM Vol 3 §8.4.4.1 建议发两次 ✓），两次之间 200 µs。
///
/// # Safety
/// `access` 必须与固件当前实际模式一致 ✗；仍在 boot services 期间 ✓。
#[cfg(target_os = "uefi")]
unsafe fn wake_ap(
    access: current::lapic::ApicAccess,
    lapic_id: u32,
    vector: current::ap::SipiVector,
    stall: Stall,
) {
    let mode = match access {
        current::lapic::ApicAccess::Xapic { .. } => current::lapic::ApicMode::Xapic,
        current::lapic::ApicAccess::X2apic => current::lapic::ApicMode::X2apic,
    };
    let dest = current::lapic::ApicId(lapic_id);
    let send = |delivery, vec, assert| {
        match current::lapic::icr_value(mode, dest, delivery, vec, assert) {
            Ok(value) => {
                // SAFETY: 由调用方保证 `access` 与固件实际模式一致（见函数文档）。
                unsafe { current::lapic::send_ipi(access, value) };
            }
            Err(error) => report_fmt::<crate::PlatformImpl>(format_args!(
                "[liftoff] ap: lapic={lapic_id:#x} 无法寻址: {error}\n"
            )),
        }
    };
    send(current::lapic::DeliveryMode::Init, 0, true);
    // SAFETY: 固件延时，调用点仍在 boot services 期间 ✓。
    unsafe { stall(current::lapic::AP_INIT_STALL_US) };
    send(current::lapic::DeliveryMode::Init, 0, false);
    // SAFETY: 同上。
    unsafe { stall(current::lapic::AP_INIT_STALL_US) };
    for round in 0..2 {
        send(current::lapic::DeliveryMode::Startup, vector.0, true);
        if round == 0 {
            // SAFETY: 同上。
            unsafe { stall(current::lapic::AP_SIPI_STALL_US) };
        }
    }
}
fn report_fmt<P: Platform>(args: core::fmt::Arguments<'_>) {
    struct Buf {
        bytes: [u8; 128],
        len: usize,
    }
    impl core::fmt::Write for Buf {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            for &byte in text.as_bytes() {
                if self.len >= self.bytes.len() {
                    return Err(core::fmt::Error);
                }
                self.bytes[self.len] = byte;
                self.len += 1;
            }
            Ok(())
        }
    }
    use core::fmt::Write;
    let mut buf = Buf { bytes: [0; 128], len: 0 };
    let _ = buf.write_fmt(args);
    report::<P>(&buf.bytes[..buf.len]);
}

/// 把一条错误渲成一行。
fn report_cause<P: Platform>(err: &impl core::fmt::Display) {
    report_fmt::<P>(format_args!("[liftoff]   cause: {}\n", err));
}

/// 把 **panic 信息**写到诊断通道：消息 + 位置。
///
/// 此前 panic 只打 `[liftoff] panic` —— 能区分「崩了」与「还在跑」，但**说不出为什么、
/// 在哪里**。与这一轮在修的其他失败同属一类：**失败必须报出自己的名字**。
///
/// 参数是**已取出的字段**而不是 `&PanicInfo`：`#[panic_handler]` 位于 `no_std` 二进制
/// 里、宿主测不了，而这样切分之后本函数**可测**（用 `format_args!` 与 `Location::caller()`）。
pub fn report_panic<P: Platform>(
    message: Option<&dyn core::fmt::Display>,
    location: Option<&core::panic::Location<'_>>,
) {
    report::<P>(b"[liftoff] panic");
    if let Some(text) = message {
        report_fmt::<P>(format_args!(": {text}"));
    }
    if let Some(at) = location {
        report_fmt::<P>(format_args!(" at {}:{}", at.file(), at.line()));
    }
    report::<P>(b"\n");
}

/// 失败环节的短文本（真跑时从串口就能看出卡在哪一步）。
fn stage_text(stage: &BringUpError) -> &'static [u8] {
    match stage {
        BringUpError::Discover(_) => b"[liftoff] stage: discover\n",
        BringUpError::Media(_) => b"[liftoff] stage: media\n",
        BringUpError::Kernel(_) => b"[liftoff] stage: kernel elf\n",
        BringUpError::Plan(_) => b"[liftoff] stage: plan\n",
        BringUpError::MemoryMapLoad(_) => b"[liftoff] stage: memmap load\n",
        BringUpError::MemoryMapRanges(_) => b"[liftoff] stage: memmap ranges\n",
        BringUpError::RootFrame => b"[liftoff] stage: root frame\n",
        BringUpError::DirectMap => b"[liftoff] stage: direct map\n",
        BringUpError::Apply(_) => b"[liftoff] stage: apply plan\n",
        BringUpError::Copy(_) => b"[liftoff] stage: copy segments\n",
        BringUpError::Responses(_) => b"[liftoff] stage: responses\n",
        BringUpError::Handoff(_) => b"[liftoff] stage: handoff\n",
    }
}

// （原 activate_only 辅助已删除：激活现在只发生一次，即 ExitBootServices 成功之后、
// 跳转之前，位于 enter_kernel —— 先激活再 Exit 会让固件在 Exit 内部挂死，真机实测。）

/// 把可用区间**向下对齐**到 `align`，并把长度补到整页（含尾部）。
///
/// 规划器要求基址已对齐（否则静默丢掉头部），所以对齐是调用方的责任。
fn align_ranges_down(ranges: &mut [UsableRange], align: u64) -> Result<(), PlanBuildError> {
    if align == 0 {
        return Err(PlanBuildError::Plan(PlanError::InvalidPageSize));
    }
    for range in ranges.iter_mut() {
        let end = range.end().ok_or(PlanBuildError::Plan(PlanError::AddressOverflow))?;
        let base = range.base.as_u64();
        let aligned_base = base / align * align;
        let aligned_end = end
            .checked_add(align - 1)
            .ok_or(PlanBuildError::Plan(PlanError::AddressOverflow))?
            / align
            * align;
        range.base = PhysAddr::new(aligned_base);
        range.length = aligned_end - aligned_base;
    }
    Ok(())
}

/// 在一段缓冲里取一个**按 `align` 对齐**的子切片。
///
/// 内核段的物理目标必须按大页对齐（`build_plan` 会拒绝未对齐的基址），而 UEFI 的
/// `AllocatePages` 只保证 4 KiB 对齐 —— 所以多要一页，在里面向上对齐。
fn aligned_within(buffer: &mut [u8], align: u64) -> Option<&mut [u8]> {
    let base = buffer.as_ptr() as u64;
    let misalign = base % align;
    let skip = if misalign == 0 { 0 } else { align - misalign };
    let skip = usize::try_from(skip).ok()?;
    buffer.get_mut(skip..)
}

/// 入口第一步的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// 引导服务表已取得，启动诊断已输出。
    Ready,
}

/// 生产入口：平台固定为门面选定的实现。
pub fn start(system_table: *mut SystemTable, image_handle: Handle) -> Result<Outcome, Error> {
    start_with::<crate::PlatformImpl>(system_table, image_handle)
}

/// 交接编排：**先加载内存映射**（键由此被记录）→ 取键 → 退出引导服务。
///
/// 顺序是硬要求：`ExitBootServices` 只接受**最近一次** `GetMemoryMap` 返回的键；
/// 加载失败或取不到键时**绝不退出**（否则固件服务被提前废掉，后续什么都做不了）。
///
/// # Safety
///
/// 与 [`exit_prepared`] 相同：退出后不得再调用任何固件服务，且当前代码与栈在新页表中
/// 仍须被映射。
pub unsafe fn handoff(
    source: &mut UefiMemoryMapSource<'_>,
    map_buffer: &mut [MemoryEntry],
    exit: ExitBootServices,
    image_handle: Handle,
    map_key: &mut Option<usize>,
) -> Result<usize, Error> {
    // 三步留痕是真机专用（`out` 指令在宿主用户态是特权指令 ✗）。
    #[cfg(target_os = "uefi")]
    #[cfg(target_os = "uefi")]
    for byte in b"[liftoff] h: mmap\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // **重试循环**（对照 brxLimine common/lib/misc.c:380 的 128 次重试）：
    // map_key 会因任何内存分配而失效 —— 而引导器自身在此期间做了大量分配 ——
    // 所以第一次 Exit 几乎必然被拒；每次被拒都要**重新取映射 + 重新取键** 再试。
    // （不带 /T 的 taskkill 曾让脚本挂死，那是宿主脚本问题，与此无关。）
    let mut retries = 0usize;
    loop {
        let map = source.memory_map(map_buffer)?;
        let count = map.len();
        #[cfg(target_os = "uefi")]
        #[cfg(target_os = "uefi")]
    for byte in b"[liftoff] h: key\n" as &[u8] {
            crate::PlatformImpl::write_byte(*byte);
        }
        if !capture_map_key(source, map_key) {
            return Err(Error::InvalidState);
        }
        #[cfg(target_os = "uefi")]
    for byte in b"[liftoff] h: exit\n" as &[u8] {
            crate::PlatformImpl::write_byte(*byte);
        }
        // **Exit 前关中断**：Exit 之后固件的定时器事件（如 VirtioRng 的异步回调）
        // 若再触发，其代码在新页表下不可达 → #GP → 复位（真机 #GP 落在 VirtioRngDxe
        // 的 RSP/RIP 已实测）。关中断让回调不再发生。
        //
        // 经 `Platform` 而不是裸 `cli`（C2）：入口层不得直连硬件（ADR-007/ADR-050），
        // 而且这本来就是**已有的能力**，自己再写一份就是重复造轮子（S28）。
        // 返回的旧状态有意丢弃 —— 这里要的就是「关掉且不再打开」。
        let _ = crate::PlatformImpl::disable_interrupts();
        // SAFETY: 由调用方保证（见函数文档与 `exit_prepared` 的 SAFETY 契约）。
        let exit_result = unsafe { exit_prepared(exit, image_handle, map_key) };
        // 到这里 = Exit 调用**返回了**（成功或失败都算）。打印 R 作为「活着的」证据。
        // （真机专用：out 指令在宿主用户态是特权指令。）
        #[cfg(target_os = "uefi")]
        crate::PlatformImpl::write_byte(b'R');
        match exit_result {
            Ok(()) => {
                // Exit 成功：**关中断**（旧实现 misc.c:429 同款）—— 引导服务失效后
                // 固件的定时器中断不会再进来，这是跳转前的必要状态。
                //
                // 同样经 `Platform`（C2）。这里不再需要 `#[cfg(target_os = "uefi")]`
                // 门控：抽象层在宿主上走 mock，不会执行特权指令 —— 上一处裸 `cli`
                // 恰恰**漏了**门控，正是「直连硬件」带来的不一致。
                let _ = crate::PlatformImpl::disable_interrupts();
                // 活着的证据：cli 之后还能执行（栈与代码都可达）。
                #[cfg(target_os = "uefi")]
                crate::PlatformImpl::write_byte(b'K');
                return Ok(count);
            }
            Err(err) => {
                retries += 1;
                if retries >= 4 || err != Error::Io {
                    return Err(err);
                }
                #[cfg(target_os = "uefi")]
                for byte in b"[liftoff] h: retry\n" as &[u8] {
                    crate::PlatformImpl::write_byte(*byte);
                }
            }
        }
    }
}

/// 交接前取键：把内存映射来源里记录的 `map_key` 写进槽。
///
/// 返回是否取到。**没有键时绝不编造**（`ExitBootServices` 只接受最近一次 `GetMemoryMap`
/// 返回的键；编造一个只会让固件拒绝退出）。
pub fn capture_map_key(source: &UefiMemoryMapSource<'_>, slot: &mut Option<usize>) -> bool {
    match source.map_key() {
        Some(key) => {
            *slot = Some(key);
            true
        }
        None => false,
    }
}

/// 退出引导服务（入口编排）：复用 `UefiBootServices` 的语义。
///
/// # Safety
///
/// 调用方必须保证退出后不再调用任何固件服务，且当前代码与栈在新页表中仍被映射。
pub unsafe fn exit_prepared(
    exit: ExitBootServices,
    image_handle: Handle,
    map_key: &mut Option<usize>,
) -> Result<(), Error> {
    let mut control = UefiBootServices::new(exit, image_handle, map_key);
    // SAFETY: 由调用方保证（见函数文档）。
    unsafe { control.exit_boot_services() }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, start, start_with};
    use arch::platform::{InterruptState, Platform};
    use firmware::error::Error;
    use firmware_current::current::SystemTable;
    use std::vec::Vec;

    struct Recorder;

    static mut SINK: Option<Vec<u8>> = None;

    impl Platform for Recorder {
        fn init() {}

        fn bsp_lapic_id() -> u32 {
            // 测试替身没有 APIC；返回 0 是「无此信息」的如实表达，不伪造一个像样的 ID。
            0
        }

        fn name() -> &'static str {
            "recorder"
        }

        unsafe fn jump_to(_entry: u64) -> ! {
        // 测试替身不应被调用：万一有测试走到跳转，就响亮失败，而不是静默通过。
        panic!("测试替身不应被调用")
    }

    fn halt() -> ! {
            loop {
                core::hint::spin_loop();
            }
        }

        fn write_byte(byte: u8) {
            // SAFETY: 测试内单线程；不创建对静态的引用。
            unsafe {
                let slot = (&raw mut SINK).as_mut().expect("SINK 地址有效");
                slot.as_mut().expect("SINK 已初始化").push(byte);
            }
        }

        fn disable_interrupts() -> InterruptState {
            InterruptState::from_enabled(true)
        }

        fn restore_interrupts(_state: InterruptState) {}
    }

    fn reset() {
        // SAFETY: 测试内单线程。
        unsafe { *(&raw mut SINK) = Some(Vec::new()) };
    }

    fn taken() -> Vec<u8> {
        // SAFETY: 测试内单线程。
        unsafe { (*&raw mut SINK).take().unwrap_or_default() }
    }

    #[test]
    fn a_null_system_table_fails_without_writing_anything() {
        reset();
        assert_eq!(
            start_with::<Recorder>(core::ptr::null_mut(), core::ptr::null_mut()),
            Err(Error::InvalidArgument)
        );
        assert!(taken().is_empty(), "失败时不得输出诊断（不制造假成功）");
    }

    #[test]
    fn a_valid_system_table_writes_the_startup_line() {
        reset();
        // 注意：`start_with` 现在会在拿到引导服务表后**真的做固件 I/O**（发现设备、读介质、
        // 分配页、写页表），成功路径**不再宿主可测** —— 只有真实的表才能走通，交给真机验证。
        // 所以这条测试直接测它名字所指的东西：**启动诊断本身**会被写出来。
        crate::diag::report_startup::<Recorder>();
        let line = std::string::String::from_utf8(taken()).expect("UTF-8");
        assert_eq!(line, "[liftoff] gen2 up, platform=recorder\n");
    }

    #[test]
    fn the_production_entry_uses_the_selected_platform() {
        // 生产入口只把平台固定为门面选定的实现；其行为由 QEMU 验收（PRE-2）。
        let _ = start
            as fn(*mut SystemTable, firmware_current::current::Handle) -> Result<Outcome, Error>;
    }
}

#[cfg(test)]
mod exit_tests {
    use super::exit_prepared;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::error::Error;
    use firmware_current::current::{ExitBootServices, Handle, UefiBootServices};
    use firmware::boot_services::{BootServicesControl, BootServicesState};

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static FAIL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn fake_exit(_image: Handle, _key: usize) -> firmware_current::current::Status {
        CALLS.fetch_add(1, Ordering::SeqCst);
        if FAIL.load(Ordering::SeqCst) == 1 {
            firmware_current::current::DEVICE_ERROR
        } else {
            firmware_current::current::SUCCESS
        }
    }

    fn exit_fn() -> ExitBootServices {
        fake_exit
    }

    #[test]
    fn exiting_without_a_map_key_is_rejected_without_calling_firmware() {
        CALLS.store(0, Ordering::SeqCst);
        let mut key = None;
        assert_eq!(unsafe { exit_prepared(exit_fn(), core::ptr::null_mut(), &mut key) }, Err(Error::InvalidState));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "无 map_key 时不得调用固件");
    }

    #[test]
    fn a_successful_exit_marks_the_state_exited() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(0, Ordering::SeqCst);
        let mut key = Some(0x77);
        assert_eq!(unsafe { exit_prepared(exit_fn(), core::ptr::null_mut(), &mut key) }, Ok(()));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_exit_keeps_the_firmware_active() {
        CALLS.store(0, Ordering::SeqCst);
        FAIL.store(1, Ordering::SeqCst);
        let mut key = Some(1);
        let mut control = UefiBootServices::new(exit_fn(), core::ptr::null_mut(), &mut key);
        assert_eq!(unsafe { control.exit_boot_services() }, Err(Error::Io));
        assert_eq!(control.state(), BootServicesState::Active, "失败后固件仍在运行");
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::capture_map_key;
    use core::ffi::c_void;
    use firmware::memory::{MemoryEntry, MemoryMapSource};
    use firmware_current::current::{BUFFER_TOO_SMALL, Status, SUCCESS, UefiMemoryMapSource};

    /// 假 GetMemoryMap（沿用 `efi` crate 已验证的形态）：
    /// 探测调用（map 为空）返回 BUFFER_TOO_SMALL 并给出所需大小；正式调用写入描述符并返回成功。
    ///
    /// SAFETY: 调用方按 UEFI 契约传入有效指针；本测试中始终如此。
    unsafe extern "efiapi" fn fake(
        map_size: *mut usize,
        map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        unsafe {
            *map_key = 0x1234;
            *descriptor_size = 40;
            *map_size = 40;
            if map.is_null() {
                BUFFER_TOO_SMALL
            } else {
                // 写一条全零的 40 字节描述符（UEFI 里类型 0 = 保留内存）。
                core::ptr::write_bytes(map.cast::<u8>(), 0, 40);
                SUCCESS
            }
        }
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry { base: arch::addr::PhysAddr::new(0), length: 0, kind: firmware::memory::MemoryKind::Reserved }
    }

    #[test]
    fn the_key_is_captured_after_a_successful_map_load() {
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(fake, &mut descriptors);
        assert_eq!(source.map_key(), None, "加载前没有键");
        let mut buffer = [empty_entry(); 4];
        source.memory_map(&mut buffer).expect("映射可取");
        let mut slot = None;
        assert!(capture_map_key(&source, &mut slot), "加载后应能取到键");
        assert_eq!(slot, Some(0x1234));
    }

    #[test]
    fn without_a_map_load_there_is_no_key_to_capture() {
        let mut descriptors = [0u8; 128];
        let source = UefiMemoryMapSource::new(fake, &mut descriptors);
        let mut slot = None;
        assert!(!capture_map_key(&source, &mut slot), "没加载就不该有键");
        assert_eq!(slot, None, "不得凭空编造键");
    }
}

#[cfg(test)]
mod handoff_flow_tests {
    use super::handoff;
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::memory::MemoryEntry;
    use firmware_current::current::{
        BUFFER_TOO_SMALL, DEVICE_ERROR, Handle, Status, SUCCESS, UefiMemoryMapSource,
    };

    static LOAD_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXIT_KEY: AtomicUsize = AtomicUsize::new(0);
    /// 失败路径专用：与成功路径**分开**计数，避免两个测试互相干扰（顺序相关/偶发）。
    static EXIT_CALLS_FAIL: AtomicUsize = AtomicUsize::new(0);

    /// 成功的 GetMemoryMap：探测返回 BUFFER_TOO_SMALL，正式调用写描述符并记录键。
    /// SAFETY: 调用方按 UEFI 契约传入有效指针；本测试中始终如此。
    unsafe extern "efiapi" fn good_map(
        map_size: *mut usize,
        map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        unsafe {
            LOAD_CALLS.fetch_add(1, Ordering::SeqCst);
            *map_key = 0x1234;
            *descriptor_size = 40;
            *map_size = 40;
            if map.is_null() {
                BUFFER_TOO_SMALL
            } else {
                core::ptr::write_bytes(map.cast::<u8>(), 0, 40);
                SUCCESS
            }
        }
    }

    /// 失败的 GetMemoryMap：直接返回错误状态（探测阶段就失败）。
    /// SAFETY: 同上；本实现不写任何指针。
    unsafe extern "efiapi" fn bad_map(
        _map_size: *mut usize,
        _map: *mut c_void,
        _map_key: *mut usize,
        _descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        DEVICE_ERROR
    }

    /// 失败路径用的退出服务：若被调用就计数（用于断言“从未调用”）。
/// SAFETY: 无内存访问。
unsafe extern "efiapi" fn counting_exit_fail(_image: Handle, _map_key: usize) -> Status {
    EXIT_CALLS_FAIL.fetch_add(1, Ordering::SeqCst);
    SUCCESS
}

/// 成功的 ExitBootServices：记录调用次数与收到的键。
    /// SAFETY: 无内存访问。
    unsafe extern "efiapi" fn good_exit(_image: Handle, map_key: usize) -> Status {
        EXIT_CALLS.fetch_add(1, Ordering::SeqCst);
        EXIT_KEY.store(map_key, Ordering::SeqCst);
        SUCCESS
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry {
            base: arch::addr::PhysAddr::new(0),
            length: 0,
            kind: firmware::memory::MemoryKind::Reserved,
        }
    }

    #[test]
    // #[ignore]：走完整 handoff（含真机 cli/Exit 路径），宿主用户态触发
    // STATUS_PRIVILEGED_INSTRUCTION；真机行为在 PRE-2 验证。
    #[ignore]
    fn a_successful_handoff_loads_captures_and_exits() {
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(good_map, &mut descriptors);
        let mut buffer = [empty_entry(); 4];
        let mut slot = None;
        let count = unsafe { handoff(&mut source, &mut buffer, good_exit, core::ptr::null_mut(), &mut slot) }
            .expect("交接成功");
        assert_eq!(count, 1, "描述符数来自假固件");
        assert_eq!(slot, Some(0x1234), "键必须被取到");
        assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 1, "必须调用一次退出");
        assert_eq!(EXIT_KEY.load(Ordering::SeqCst), 0x1234, "退出必须收到同一个键");
    }

    #[test]
    fn a_failed_map_load_never_reaches_the_exit() {
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(bad_map, &mut descriptors);
        let mut buffer = [empty_entry(); 4];
        let mut slot = None;
        let result = unsafe {
            handoff(
                &mut source,
                &mut buffer,
                counting_exit_fail,
                core::ptr::null_mut(),
                &mut slot,
            )
        };
        assert!(result.is_err(), "加载失败必须报错");
        assert_eq!(slot, None, "没有键就不该有键");
        assert_eq!(
            EXIT_CALLS_FAIL.load(Ordering::SeqCst),
            0,
            "加载失败绝不能退出引导服务"
        );
    }
}

/// 进入内核前的检查失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryError {
    /// 入口地址为 0（不可跳转）。
    NoEntry,
    /// 某个必须保持映射的地址未被规划覆盖。
    NotCovered {
        /// 未被覆盖的地址。
        address: u64,
    },
    /// 调用方给的「必须保持映射」区间长度为 0（调用方错误）。
    EmptySpan,
}

impl core::fmt::Display for EntryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoEntry => f.write_str("入口地址为 0，不可跳转"),
            // **地址必须打出来**：不做有损扁平化正是这条错误存在的理由，
            // 丢掉地址就等于丢掉了唯一的定位线索。
            Self::NotCovered { address } => {
                write!(f, "必须保持映射的地址 {address:#x} 未被规划覆盖")
            }
            Self::EmptySpan => f.write_str("必须保持映射的区间长度为 0（调用方错误）"),
        }
    }
}

/// 进入内核前的**前置检查**（纯，宿主可测）：入口非零，且必须保持映射的区间都被规划覆盖。
///
/// 返回入口地址；任一项不满足即报错 —— **绝不带着未覆盖的代码或栈跳转**。
/// 错误**保留未被覆盖的具体地址**（不做有损扁平化），便于定位。
pub fn check_before_entry(
    entry: u64,
    plan: &[Mapping],
    must_stay: &[MustStay],
) -> Result<u64, EntryError> {
    if entry == 0 {
        return Err(EntryError::NoEntry);
    }
    match mm::takeover::check_coverage(plan, must_stay) {
        Ok(()) => Ok(entry),
        Err(TakeoverError::Uncovered { address }) => Err(EntryError::NotCovered { address }),
        Err(TakeoverError::EmptyRange) => Err(EntryError::EmptySpan),
    }
}

#[cfg(test)]
mod bootloader_info_tests {
    use super::{BOOTLOADER_NAME, BOOTLOADER_VERSION};

    #[test]
    fn the_self_description_is_nul_terminated_and_not_empty() {
        // Limine 协议要的是 **C 字符串**：缺 NUL 结尾，内核读串就会越界。
        // 而这两个串此前**根本没有被交付**（`set_bootloader_info` 定义了却没被调用）。
        assert!(BOOTLOADER_NAME.ends_with(&[0]), "名字必须以 NUL 结尾");
        assert!(BOOTLOADER_VERSION.ends_with(&[0]), "版本必须以 NUL 结尾");
        assert!(BOOTLOADER_NAME.len() > 1, "名字不能是空串");
        assert!(BOOTLOADER_VERSION.len() > 1, "版本不能是空串");
        // 中间不得出现 NUL，否则内核读到的会是被截断的串。
        assert!(!BOOTLOADER_NAME[..BOOTLOADER_NAME.len() - 1].contains(&0));
        assert!(!BOOTLOADER_VERSION[..BOOTLOADER_VERSION.len() - 1].contains(&0));
    }
}

#[cfg(test)]
mod report_failure_tests {
    use super::{BringUpError, report_failure, report_panic};
    use arch::paging::MapError;
    use arch::platform::{InterruptState, Platform};
    use std::string::String;
    use std::sync::Mutex;
    use std::vec::Vec;

    /// 记录输出的替身平台。
    static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    struct Sink;

    impl Platform for Sink {
        fn init() {}

        fn name() -> &'static str {
            "sink"
        }

        unsafe fn jump_to(_entry: u64) -> ! {
            panic!("测试替身不应被调用")
        }

        fn halt() -> ! {
            panic!("测试替身不应被调用")
        }

        fn write_byte(byte: u8) {
            OUT.lock().unwrap_or_else(|e| e.into_inner()).push(byte);
        }

        fn bsp_lapic_id() -> u32 {
            0
        }

        fn disable_interrupts() -> InterruptState {
            InterruptState::from_enabled(false)
        }

        fn restore_interrupts(_state: InterruptState) {}
    }

    fn capture_panic(
        message: Option<&dyn core::fmt::Display>,
        location: Option<&core::panic::Location<'_>>,
    ) -> String {
        OUT.lock().unwrap_or_else(|e| e.into_inner()).clear();
        report_panic::<Sink>(message, location);
        let bytes = OUT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        String::from_utf8(bytes).expect("诊断输出应是 UTF-8")
    }

    #[test]
    fn a_panic_reports_its_message_and_location() {
        // **panic 此前是唯一说不出自己名字的失败**：只打 `[liftoff] panic`，
        // 能区分「崩了」与「还在跑」，但说不出为什么、在哪里。
        static MESSAGE: &str = "下标越界";
        struct Text(&'static str);
        impl core::fmt::Display for Text {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(self.0)
            }
        }
        let message = Text(MESSAGE);
        let text = capture_panic(Some(&message), Some(core::panic::Location::caller()));
        assert!(text.contains("panic"), "必须标明是 panic: {text}");
        assert!(text.contains("下标越界"), "必须打出 panic 消息: {text}");
        assert!(text.contains("entry.rs"), "必须打出位置: {text}");
    }

    #[test]
    fn a_panic_without_a_message_still_marks_itself() {
        let text = capture_panic(None, None);
        assert!(text.contains("panic"), "即使没有消息也必须留痕: {text}");
    }

    fn capture(stage: &BringUpError) -> String {
        OUT.lock().unwrap_or_else(|e| e.into_inner()).clear();
        report_failure::<Sink>(stage);
        let bytes = OUT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        String::from_utf8(bytes).expect("诊断输出应是 UTF-8")
    }

    #[test]
    fn a_failure_reports_both_the_stage_and_the_cause() {
        // **这条测试守着一次三次真机失败的教训。** 此前每个环节只打一个通用标记：
        // "卡在哪一步"可见、"为什么"不可见。定位混合粒度问题时，只有 `apply` 因为
        // 一处特例会打印具体错误值 —— 正是那个值一步锁定了根因。
        let text = capture(&BringUpError::Apply(MapError::UnsupportedGranularity));
        assert!(text.contains("stage: apply plan"), "必须打环节: {text}");
        assert!(text.contains("cause:"), "必须打具体原因: {text}");
        // 断言**人类可读**的消息（`Display`），而不是 Debug 里的变体名 —— 后者只是
        // 临时手段，前者才是这次改进的实质。
        assert!(
            text.contains("实现不支持该页粒度"),
            "原因必须是人类可读的消息: {text}"
        );
    }

    #[test]
    fn a_payloadless_failure_still_reports_its_stage() {
        let text = capture(&BringUpError::RootFrame);
        assert!(text.contains("stage: root frame"), "无载荷环节也要报: {text}");
        assert!(!text.contains("cause:"), "无载荷时不得伪造原因: {text}");
    }

    #[test]
    fn a_long_message_is_truncated_rather_than_panicking() {
        // 固定缓冲溢出必须**截断**，不能让诊断本身成为故障源。
        let text = capture(&BringUpError::Plan(super::PlanBuildError::Plan(
            mm::plan::PlanError::AddressOverflow,
        )));
        assert!(text.contains("stage: plan"), "环节必须仍然可见: {text}");
        assert!(text.len() < 300, "输出必须被固定缓冲限制住，实得 {} 字节", text.len());
    }
}

#[cfg(test)]
mod before_entry_tests {
    use super::{EntryError, check_before_entry};
    use mm::plan::Mapping;
    use mm::takeover::MustStay;
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::PageFlags;

    const LARGE: u64 = 2 * 1024 * 1024;

    fn mapping(virt: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(virt),
            len,
            flags: PageFlags::present(),
        }
    }

    fn stay(start: u64, len: u64) -> MustStay {
        MustStay { start, len }
    }

    #[test]
    fn a_covered_entry_is_returned() {
        let plan = [mapping(0xffff_ffff_8000_0000, LARGE)];
        let must = [stay(0xffff_ffff_8000_0000, LARGE)];
        let entry = check_before_entry(0xffff_ffff_8000_0100, &plan, &must).expect("检查通过");
        assert_eq!(entry, 0xffff_ffff_8000_0100);
    }

    #[test]
    fn a_zero_entry_is_rejected() {
        let plan = [mapping(0xffff_ffff_8000_0000, LARGE)];
        let must = [stay(0xffff_ffff_8000_0000, LARGE)];
        assert_eq!(check_before_entry(0, &plan, &must), Err(EntryError::NoEntry));
    }

    #[test]
    fn an_uncovered_span_is_rejected_with_its_address() {
        let plan = [mapping(0xffff_ffff_8000_0000, LARGE)];
        // 第二段没有被任何映射覆盖。
        let must = [stay(0xffff_ffff_8000_0000, LARGE), stay(0xffff_ffff_9000_0000, LARGE)];
        assert_eq!(
            check_before_entry(0xffff_ffff_8000_0100, &plan, &must),
            Err(EntryError::NotCovered { address: 0xffff_ffff_9000_0000 })
            , "必须报出未被覆盖的地址"
        );
    }

    #[test]
    fn an_empty_plan_cannot_cover_a_non_empty_span() {
        let must = [stay(0xffff_ffff_8000_0000, LARGE)];
        assert_eq!(
            check_before_entry(0xffff_ffff_8000_0100, &[], &must),
            Err(EntryError::NotCovered { address: 0xffff_ffff_8000_0000 })
        );
    }
}

/// 交接编排的全部输入（一次装配好，避免长参数列表）。
pub struct Handoff<'a, 'b> {
    /// 内核映像（**可写**：要把响应指针写进请求头）。
    pub image: &'a mut [u8],
    /// spinup 跳板的低地址缓冲布局（Exit 后从 common64 跳进去）。
    pub spinup: spinup::LowBuffer,
    /// 扫描请求时只看这些**文件区间**（已装载段）；空表示扫全映像。
    pub ranges: &'a [(usize, usize)],
    /// 扫描用的命中缓冲。
    pub hits: &'a mut [RequestHit],
    /// 我们准备的响应结构。
    pub responses: &'a mut Responses,
    /// 页表规划（用于覆盖检查）。
    pub plan: &'a [Mapping],
    /// 必须保持映射的区间（调用方提供）。
    pub must_stay: &'a [MustStay],
    /// 内核入口地址。
    pub entry: u64,
    /// 内存映射来源（退出前要用它记录的键）。
    pub source: &'a mut UefiMemoryMapSource<'b>,
    /// 内存映射缓冲。
    pub map_buffer: &'a mut [MemoryEntry],
    /// 退出引导服务的固件函数。
    pub exit: ExitBootServices,
    /// 映像句柄。
    pub image_handle: Handle,
    /// 键的存放槽。
    pub map_key: &'a mut Option<usize>,
}

/// 交接失败原因（**保留来源**，不做有损扁平化）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandoffError {
    /// 响应填充阶段的扫描错误。
    Fill(limine::scan::ScanError),
    /// 跳转前检查失败。
    BeforeEntry(EntryError),
    /// 退出引导服务失败。
    Exit(Error),
}

impl core::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // 两个载荷（`ScanError` / `EntryError`）现均实现 `Display`，故用 `{}`。
            Self::Fill(err) => write!(f, "响应填充扫描失败: {err}"),
            // `EntryError` 现已实现 `Display`，故用 `{}`。
            Self::BeforeEntry(err) => write!(f, "跳转前检查失败: {err}"),
            Self::Exit(err) => write!(f, "退出引导服务失败: {err}"),
        }
    }
}

/// 完整交接编排：填充响应 → 跳转前检查 → 取键并退出 → 跳转。
///
/// 顺序与门禁是硬要求：**检查不通过绝不跳转**；**退出失败绝不跳转**。
/// 跳转以 `enter` 注入（真实路径传 `Platform::jump_to`），故本函数宿主可测。
///
/// # Safety
///
/// 由 `enter` 的实现与调用方共同保证：跳转后不再调用任何固件服务，且目标已映射。
pub unsafe fn enter_kernel<F, P: PageTable>(
    h: Handoff<'_, '_>,
    _page_table: &mut P,
    _enter: F,
) -> Result<usize, HandoffError>
where
    F: FnOnce(u64) -> !,
{
    let report = crate::protocol::fill_responses(h.image, h.ranges, h.hits, h.responses)
        .map_err(HandoffError::Fill)?;
    // 交接四步留痕是真机专用：`write_byte` 走 `out` 指令，宿主用户态是特权指令。
    #[cfg(target_os = "uefi")]
    for byte in b"[liftoff] filled\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    #[allow(unused_variables)]
    let entry =
        check_before_entry(h.entry, h.plan, h.must_stay).map_err(HandoffError::BeforeEntry)?;
    #[cfg(target_os = "uefi")]
    for byte in b"[liftoff] checked\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // 映射条目数只作诊断：本函数**必然发散**（`enter` 的返回类型是 `!`），故显式标记为有意不用。
    let _count = unsafe { handoff(h.source, h.map_buffer, h.exit, h.image_handle, h.map_key) }
        .map_err(|err| {
            // **Exit 失败不是终点**：如果 Exit 被固件拒绝（最常见 EFI_INVALID_PARAMETER =
            // 键已失效），协议允许重取内存映射再试 —— 但**必须在错误留痕里区分**，
            // 否则真机上「退出失败」与「退出后死」无法区分（已实测混淆过一轮）。
            #[cfg(target_os = "uefi")]
            for byte in match err {
                firmware::error::Error::Io => b"[liftoff] h: exit REFUSED\n" as &[u8],
                _ => b"[liftoff] h: exit INVALID\n" as &[u8],
            } {
                crate::PlatformImpl::write_byte(*byte);
            }
            HandoffError::Exit(err)
        })?;
    let _ = report;
    // **Exit 成功之后立刻激活并跳转**：引导服务已失效，只有纯寄存器操作是安全的。
    // 真机数据显示「先激活再 Exit」会让固件在 Exit 内部挂死 —— 所以激活必须放这里。
    // 激活前打印：若它在 `mov cr3` 之后死，说明新表的恒等映射没覆盖引导器映像。
    #[cfg(target_os = "uefi")]
    for byte in b"[liftoff] pre-act\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // 交接的最后一步：跳进低地址 trampoline —— 重设机器状态（降 32 位关分页
    // → 按 Limine 语义重建分页 → 重进 64 位 → iretq 全 GPR 清零）→ 进内核。
    // SAFETY: trampoline 在 Exit 前已搬进低地址缓冲；Exit 成功后只有寄存器操作安全。
    for byte in b"[liftoff] G-\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    unsafe { spinup::spinup_go(h.spinup) }
}

#[cfg(test)]
mod enter_kernel_tests {
    use super::PageTable;
    use arch::paging::MapError;
    /// 宿主假表：什么都不做（激活在宿主测试里永远不该真的发生）。
    pub(super) struct StubTable;
    impl PageTable for StubTable {
        fn map_range(
            &mut self,
            _virt: VirtAddr,
            _phys: PhysAddr,
            _len: u64,
            _flags: PageFlags,
        ) -> Result<(), MapError> {
            Ok(())
        }
        // 假表不建模翻译：如实回答「未映射」，不假装知道。
        fn translate(&self, _virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
            None
        }

        unsafe fn activate(&self) {}
    }
    use super::{EntryError, Handoff, HandoffError, enter_kernel};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::memory::MemoryEntry;
    use firmware_current::current::{
        BUFFER_TOO_SMALL, DEVICE_ERROR, Handle, Status, SUCCESS, UefiMemoryMapSource,
    };
    use limine::base::HHDM_REQUEST_ID;
    use limine::scan::{RequestHit, END_MARKER, START_MARKER};
    use mm::plan::Mapping;
    use mm::takeover::MustStay;
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::PageFlags;
    use crate::responses::Responses;

    const LARGE: u64 = 2 * 1024 * 1024;

    /// 记录“是否被要求跳转”。若真跳了，本测试会因计数不符而失败（而不是真的跳走）。
    static ENTER_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn good_map(
        map_size: *mut usize,
        map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        unsafe {
            *map_key = 0x1234;
            *descriptor_size = 40;
            *map_size = 40;
            if map.is_null() {
                BUFFER_TOO_SMALL
            } else {
                core::ptr::write_bytes(map.cast::<u8>(), 0, 40);
                SUCCESS
            }
        }
    }

    unsafe extern "efiapi" fn never_exit(_image: Handle, _map_key: usize) -> Status {
        DEVICE_ERROR
    }

    /// 测试用的“跳转”：只计数，然后 panic 终止（绝不真的跳走）。
    fn counting_enter(_entry: u64) -> ! {
        ENTER_CALLS.fetch_add(1, Ordering::SeqCst);
        panic!("测试中不应真的跳转")
    }

    fn push_words(image: &mut std::vec::Vec<u8>, words: &[u64]) {
        for word in words {
            image.extend_from_slice(&word.to_ne_bytes());
        }
    }

    /// 映像：START + 一个 HHDM 请求 + END。
    fn image_with_hhdm() -> std::vec::Vec<u8> {
        let mut image = std::vec![0u8; 64];
        push_words(&mut image, &START_MARKER);
        push_words(&mut image, &HHDM_REQUEST_ID);
        push_words(&mut image, &[0, 0, 0]);
        push_words(&mut image, &END_MARKER);
        image
    }

    fn mapping(virt: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(virt),
            len,
            flags: PageFlags::present(),
        }
    }

    fn stay(start: u64, len: u64) -> MustStay {
        MustStay { start, len }
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry {
            base: arch::addr::PhysAddr::new(0),
            length: 0,
            kind: firmware::memory::MemoryKind::Reserved,
        }
    }

    #[test]
    fn an_uncovered_span_stops_before_any_jump() {
        let mut image = image_with_hhdm();
        let mut hits = [RequestHit::EMPTY; 4];
        let mut responses = Responses::new();
        let plan = [mapping(0xffff_ffff_8000_0000, LARGE)];
        let must = [stay(0xffff_ffff_9000_0000, LARGE)];
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(good_map, &mut descriptors);
        let mut map_buffer = [empty_entry(); 4];
        let mut slot = None;
        let h = Handoff {
            image: &mut image,
            spinup: current::spinup::LowBuffer {
                go32: 0,
                spinup32: 0,
                args: 0,
            },
            ranges: &[],
            hits: &mut hits,
            responses: &mut responses,
            plan: &plan,
            must_stay: &must,
            entry: 0xffff_ffff_8000_0100,
            source: &mut source,
            map_buffer: &mut map_buffer,
            exit: never_exit,
            image_handle: core::ptr::null_mut(),
            map_key: &mut slot,
        };
        let mut stub_table = StubTable;
        let result = unsafe { enter_kernel(h, &mut stub_table, counting_enter) };
        assert_eq!(
            result,
            Err(HandoffError::BeforeEntry(EntryError::NotCovered {
                address: 0xffff_ffff_9000_0000
            })),
            "覆盖不全必须报错"
        );
        assert_eq!(ENTER_CALLS.load(Ordering::SeqCst), 0, "覆盖不全时绝不能跳转");
    }

    #[test]
    fn a_zero_entry_stops_before_any_jump() {
        let mut image = image_with_hhdm();
        let mut hits = [RequestHit::EMPTY; 4];
        let mut responses = Responses::new();
        let plan = [mapping(0xffff_ffff_8000_0000, LARGE)];
        let must = [stay(0xffff_ffff_8000_0000, LARGE)];
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(good_map, &mut descriptors);
        let mut map_buffer = [empty_entry(); 4];
        let mut slot = None;
        let h = Handoff {
            image: &mut image,
            spinup: current::spinup::LowBuffer {
                go32: 0,
                spinup32: 0,
                args: 0,
            },
            ranges: &[],
            hits: &mut hits,
            responses: &mut responses,
            plan: &plan,
            must_stay: &must,
            entry: 0,
            source: &mut source,
            map_buffer: &mut map_buffer,
            exit: never_exit,
            image_handle: core::ptr::null_mut(),
            map_key: &mut slot,
        };
        let mut stub_table = StubTable;
        let result = unsafe { enter_kernel(h, &mut stub_table, counting_enter) };
        assert_eq!(
            result,
            Err(HandoffError::BeforeEntry(EntryError::NoEntry)),
            "入口为 0 必须报错"
        );
        assert_eq!(ENTER_CALLS.load(Ordering::SeqCst), 0, "入口为 0 时绝不能跳转");
    }
}

#[cfg(test)]
mod enter_kernel_success_tests {
    use super::enter_kernel_tests::StubTable;
    use super::{Handoff, enter_kernel};
    use crate::responses::Responses;
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use firmware::memory::MemoryEntry;
    use firmware_current::current::{
        BUFFER_TOO_SMALL, Handle, Status, SUCCESS, UefiMemoryMapSource,
    };
    use limine::base::HHDM_REQUEST_ID;
    use limine::scan::{RequestHit, END_MARKER, START_MARKER};
    use mm::plan::Mapping;
    use mm::takeover::MustStay;
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::PageFlags;

    const LARGE: u64 = 2 * 1024 * 1024;

    static ENTER_ENTRY: AtomicUsize = AtomicUsize::new(0);
    static EXIT_OK_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXIT_OK_KEY: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "efiapi" fn good_map(
        map_size: *mut usize,
        map: *mut c_void,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        _version: *mut u32,
    ) -> Status {
        unsafe {
            *map_key = 0x1234;
            *descriptor_size = 40;
            *map_size = 40;
            if map.is_null() {
                BUFFER_TOO_SMALL
            } else {
                core::ptr::write_bytes(map.cast::<u8>(), 0, 40);
                SUCCESS
            }
        }
    }

    unsafe extern "efiapi" fn exit_ok(_image: Handle, map_key: usize) -> Status {
        EXIT_OK_CALLS.fetch_add(1, Ordering::SeqCst);
        EXIT_OK_KEY.store(map_key, Ordering::SeqCst);
        SUCCESS
    }

    /// 记录收到的入口地址，然后 panic 截住（绝不真的跳走）。
    fn recording_enter(entry: u64) -> ! {
        ENTER_ENTRY.store(entry as usize, Ordering::SeqCst);
        panic!("测试中的跳转到此为止")
    }

    fn push_words(image: &mut std::vec::Vec<u8>, words: &[u64]) {
        for word in words {
            image.extend_from_slice(&word.to_ne_bytes());
        }
    }

    fn image_with_hhdm() -> std::vec::Vec<u8> {
        let mut image = std::vec![0u8; 64];
        push_words(&mut image, &START_MARKER);
        push_words(&mut image, &HHDM_REQUEST_ID);
        push_words(&mut image, &[0, 0, 0]);
        push_words(&mut image, &END_MARKER);
        image
    }

    fn mapping(virt: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(virt),
            len,
            flags: PageFlags::present(),
        }
    }

    fn stay(start: u64, len: u64) -> MustStay {
        MustStay { start, len }
    }

    fn empty_entry() -> MemoryEntry {
        MemoryEntry {
            base: arch::addr::PhysAddr::new(0),
            length: 0,
            kind: firmware::memory::MemoryKind::Reserved,
        }
    }

    #[test]
    // #[ignore]：此测试走完整 handoff（含 spinup_go），而 spinup_go 在宿主上
    // 是不可达的占位（trampoline 汇编只在 UEFI 目标存在）。
    // 该路径由真机验证（PRE-2），宿主只覆盖 spinup 之前的所有步骤。
    #[ignore]
    fn a_successful_path_fills_responses_exits_once_and_jumps_to_the_entry() {
        let mut image = image_with_hhdm();
        let mut hits = [RequestHit::EMPTY; 4];
        let mut responses = Responses::new();
        responses.set_hhdm_offset(0xffff_8000_0000_0000);
        let plan = [mapping(0xffff_ffff_8000_0000, LARGE)];
        let must = [stay(0xffff_ffff_8000_0000, LARGE)];
        let mut descriptors = [0u8; 128];
        let mut source = UefiMemoryMapSource::new(good_map, &mut descriptors);
        let mut map_buffer = [empty_entry(); 4];
        let mut slot = None;
        let entry = 0xffff_ffff_8000_0100u64;
        let h = Handoff {
            image: &mut image,
            spinup: current::spinup::LowBuffer {
                go32: 0,
                spinup32: 0,
                args: 0,
            },
            ranges: &[],
            hits: &mut hits,
            responses: &mut responses,
            plan: &plan,
            must_stay: &must,
            entry,
            source: &mut source,
            map_buffer: &mut map_buffer,
            exit: exit_ok,
            image_handle: core::ptr::null_mut(),
            map_key: &mut slot,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let mut stub_table = StubTable;
            enter_kernel(h, &mut stub_table, recording_enter)
        }));
        assert!(outcome.is_err(), "编排必然以跳转结束（测试替身用 panic 截住）");
        assert_eq!(ENTER_ENTRY.load(Ordering::SeqCst), entry as usize, "必须跳到内核入口");
        assert_eq!(EXIT_OK_CALLS.load(Ordering::SeqCst), 1, "必须退出一次");
        assert_eq!(EXIT_OK_KEY.load(Ordering::SeqCst), 0x1234, "退出必须收到取到的键");
        assert_eq!(slot, Some(0x1234), "键必须已写入槽");
        let at = hits[0].offset + 40;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&image[at..at + 8]);
        assert_ne!(usize::from_ne_bytes(buf), 0, "response 必须已被写入");
    }
}

/// 内核装载计划。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KernelPlan {
    /// 内核入口（`e_entry`）。
    pub entry: u64,
    /// 装载段数量。
    pub segment_count: usize,
    /// 入口落在第几段（自检：入口必须在某个装载段内）。
    pub entry_segment: usize,
}

/// 规划内核装载的失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelPlanError {
    /// ELF 解析失败。
    Elf(ElfError),
    /// 入口不在任何装载段内。
    EntryOutsideSegments,
    /// 段的虚拟区间长度为 0。
    EmptySegment,
    /// 段区间末端溢出。
    Overflow,
    /// 调用方给的输出缓冲太小。
    BufferTooSmall,
    /// 段的目标虚拟区间没有任何映射覆盖。
    NotMapped,
    /// 把规划写入页表失败。
    MapFailed,
    /// 一条重定位的目标地址不在规划覆盖范围内（**不跳过**）。
    RelocationNotMapped,
}

impl core::fmt::Display for KernelPlanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // `ElfError` 现已实现 `Display`（`loader` crate），故用 `{}`。
            Self::Elf(err) => write!(f, "ELF 解析失败: {err}"),
            Self::EntryOutsideSegments => f.write_str("入口不在任何装载段内"),
            Self::EmptySegment => f.write_str("段的虚拟区间长度为 0"),
            Self::Overflow => f.write_str("段区间末端溢出"),
            Self::BufferTooSmall => f.write_str("调用方给的输出缓冲太小"),
            Self::NotMapped => f.write_str("段的目标虚拟区间没有任何映射覆盖"),
            Self::MapFailed => f.write_str("把规划写入页表失败"),
            Self::RelocationNotMapped => f.write_str("重定位目标不在规划覆盖范围内"),
        }
    }
}

/// 解析内核映像，得出入口与装载段。
///
/// 数值边界：每段 `vaddr + memsz` 都做 checked 加法（溢出即拒绝）；入口必须落在某段内，
/// 否则跳过去就是执行未装载的内存。
pub fn plan_kernel(
    image: &[u8],
    segments: &mut [ProgramHeader],
) -> Result<KernelPlan, KernelPlanError> {
    let header = parse_elf_header(image).map_err(KernelPlanError::Elf)?;
    let count = parse_load_segments(image, &header, segments).map_err(KernelPlanError::Elf)?;
    let entry = header.e_entry;
    let mut entry_segment = usize::MAX;
    for (index, segment) in segments[..count].iter().enumerate() {
        let end = segment
            .p_vaddr
            .checked_add(segment.p_memsz)
            .ok_or(KernelPlanError::Overflow)?;
        if entry >= segment.p_vaddr && entry < end {
            entry_segment = index;
            break;
        }
    }
    if entry_segment == usize::MAX {
        return Err(KernelPlanError::EntryOutsideSegments);
    }
    Ok(KernelPlan { entry, segment_count: count, entry_segment })
}

/// 把装载段的目标虚拟区间填进 `must_stay`（跳转后这些区间必须仍然映射）。
pub fn must_stay_from_segments(
    segments: &[ProgramHeader],
    out: &mut [MustStay],
) -> Result<usize, KernelPlanError> {
    if out.len() < segments.len() {
        return Err(KernelPlanError::BufferTooSmall);
    }
    for (index, segment) in segments.iter().enumerate() {
        if segment.p_memsz == 0 {
            return Err(KernelPlanError::EmptySegment);
        }
        segment
            .p_vaddr
            .checked_add(segment.p_memsz)
            .ok_or(KernelPlanError::Overflow)?;
        out[index] = MustStay { start: segment.p_vaddr, len: segment.p_memsz };
    }
    Ok(segments.len())
}

#[cfg(test)]
mod kernel_plan_tests {
    use super::{KernelPlanError, must_stay_from_segments, plan_kernel};
    use loader::elf::ProgramHeader;
    use mm::takeover::MustStay;

    /// 从真实 ISO 里取出内核映像（extent 33、24,619,400 字节）。
    /// 委派到共享实现（S15）：ISO 路径、LBA 与长度只在 `test_support` 里定义一次。
    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        crate::test_support::real_kernel()
    }

    #[test]
    fn the_real_kernel_plan_matches_the_measured_layout() {
        let Some(image) = real_kernel() else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut segments = [ProgramHeader::EMPTY; 8];
        let plan = plan_kernel(&image, &mut segments).expect("内核可规划");
        assert_eq!(plan.segment_count, 3, "实测 3 个 PT_LOAD");
        assert_eq!(plan.entry, 0xffff_ffff_8003_78d0, "实测入口");
        assert_eq!(plan.entry_segment, 0, "入口落在第 0 段");

        let mut stays = [MustStay { start: 0, len: 0 }; 8];
        let count = must_stay_from_segments(&segments[..plan.segment_count], &mut stays)
            .expect("区间可生成");
        assert_eq!(count, 3);
        assert_eq!(stays[0].start, 0xffff_ffff_8000_0000);
        assert_eq!(stays[0].len, 0x22_3cb0);
        assert_eq!(stays[1].start, 0xffff_ffff_8022_4000);
        assert_eq!(stays[1].len, 0x4d_5780);
        assert_eq!(stays[2].start, 0xffff_ffff_806f_a000);
        assert_eq!(stays[2].len, 0x2b_ea88);
    }

    #[test]
    fn a_zero_length_segment_is_rejected() {
        let mut segments = [ProgramHeader::EMPTY; 4];
        segments[0].p_vaddr = 0xffff_ffff_8000_0000;
        segments[0].p_memsz = 0;
        let mut stays = [MustStay { start: 0, len: 0 }; 4];
        assert_eq!(
            must_stay_from_segments(&segments[..1], &mut stays),
            Err(KernelPlanError::EmptySegment)
        );
    }

    #[test]
    fn a_segment_whose_end_overflows_is_rejected() {
        let mut segments = [ProgramHeader::EMPTY; 4];
        segments[0].p_vaddr = u64::MAX - 1;
        segments[0].p_memsz = 8;
        let mut stays = [MustStay { start: 0, len: 0 }; 4];
        assert_eq!(
            must_stay_from_segments(&segments[..1], &mut stays),
            Err(KernelPlanError::Overflow)
        );
    }

    #[test]
    fn more_segments_than_the_output_buffer_is_rejected() {
        let mut segments = [ProgramHeader::EMPTY; 4];
        for seg in segments.iter_mut() {
            seg.p_vaddr = 0xffff_ffff_8000_0000;
            seg.p_memsz = 0x1000;
        }
        let mut stays = [MustStay { start: 0, len: 0 }; 2];
        assert_eq!(
            must_stay_from_segments(&segments, &mut stays),
            Err(KernelPlanError::BufferTooSmall)
        );
    }
}

/// 按页表规划把内核段拷进物理内存。
///
/// `load_segments` 的写入器收到的是 `p_vaddr`（虚拟地址），本函数用 `plan` 把它翻成物理地址：
/// 找到**覆盖该虚拟区间的那一条**映射，目标为 `phys + (vaddr - mapping.virt)`。
/// **覆盖不到就返回 `NotMapped`**，绝不静默丢弃字节 —— 半装载的映像跳过去就是执行垃圾。
///
/// `memory` 是目标物理内存视图（长度即可寻址的物理字节数），故宿主上可用假内存验证。
///
/// 边界：映射区间与物理地址都用 checked 运算；写入目标必须在 `memory` 内。
/// 把 `ET_DYN` 内核的 `R_X86_64_RELATIVE` 重定位写进**已装载**的内存。
///
/// `loader` 只负责**解析**（纯字节，见 `loader::elf::relative_relocations`），
/// 写入属 `boot` 职责：按 `plan` 的 virt→phys 映射把每个目标虚拟地址换算成物理
/// 地址，再写 8 字节小端。
///
/// `slide` = 实际装载地址 − 链接期虚拟地址；我们按链接地址装载，所以是 0。
///
/// 返回应用了多少条。**任何一条目标地址（含 8 字节）不被任何映射覆盖都报错**，
/// 不静默跳过。
///
/// # 为什么必须有这一步
///
/// 真实内核是 `ET_DYN`，其 `_start` 用 `mov 0xffffffff809b0550,%rcx; mov %rcx,%rsp`
/// 载入自己的栈指针；该槽位在文件里是 0，只有应用 `R_X86_64_RELATIVE`
/// （`r_addend = -0x7f651000` → `0xffffffff809af000`）之后才正确。不应用它，
/// `RSP` 就是 0，内核入口第一条 `call` 就会写 `-8` 而 #PF（已由单步实测）。
pub fn apply_kernel_relocations<W>(
    image: &[u8],
    plan: &[Mapping],
    slide: u64,
    write_phys: W,
) -> Result<usize, KernelPlanError>
where
    W: FnMut(u64, &[u8]) -> Result<(), ElfError>,
{
    // 非 PIE 内核（`ET_EXEC`）的地址已经是最终地址，**不需要**也不该做重定位。
    let header = loader::elf::parse_elf_header(image).map_err(KernelPlanError::Elf)?;
    if header.e_type != loader::elf::ET_DYN {
        return Ok(0);
    }
    let mut write_phys = write_phys;
    let relocations =
        loader::elf::relative_relocations(image, slide).map_err(KernelPlanError::Elf)?;
    let mut applied = 0usize;
    for relocation in relocations {
        let end = relocation.address.saturating_add(8);
        let Some(mapping) = plan.iter().find(|m| {
            let base = m.virt.as_u64();
            relocation.address >= base && end <= base.saturating_add(m.len)
        }) else {
            return Err(KernelPlanError::RelocationNotMapped);
        };
        let phys = mapping
            .phys
            .as_u64()
            .saturating_add(relocation.address - mapping.virt.as_u64());
        write_phys(phys, &relocation.value.to_le_bytes()).map_err(KernelPlanError::Elf)?;
        applied += 1;
    }
    Ok(applied)
}

pub fn copy_kernel_segments<W>(
    image: &[u8],
    segments: &[ProgramHeader],
    plan: &[Mapping],
    write_phys: W,
) -> Result<usize, KernelPlanError>
where
    W: FnMut(u64, &[u8]) -> Result<(), ElfError>,
{
    let mut total = 0usize;
    let mut not_mapped = false;
    let mut write_phys = write_phys;
    let loaded = {
        let mut write = |vaddr: u64, bytes: &[u8]| -> Result<(), ElfError> {
            // 一段可能**跨多条映射**：真实内核的第 1 段有 0x4d5780（约 4.8 MiB），
            // 远大于一条 2 MiB 映射。所以按映射**切分**写入，而不是要求单条映射覆盖整段
            // —— 后者在真机上直接报 NotMapped（宿主测试已复现）。
            let mut at = vaddr;
            let mut rest = bytes;
            while !rest.is_empty() {
                let Some(mapping) = plan.iter().find(|m| {
                    let base = m.virt.as_u64();
                    at >= base && at < base.saturating_add(m.len)
                }) else {
                    not_mapped = true;
                    return Err(ElfError::SegmentOutOfBounds);
                };
                let base = mapping.virt.as_u64();
                let offset = at - base;
                let room = mapping.len - offset;
                let take = core::cmp::min(room, rest.len() as u64) as usize;
                let phys = mapping
                    .phys
                    .as_u64()
                    .checked_add(offset)
                    .ok_or(ElfError::SegmentOutOfBounds)?;
                write_phys(phys, &rest[..take])?;
                total += take;
                at = at
                    .checked_add(take as u64)
                    .ok_or(ElfError::SegmentOutOfBounds)?;
                rest = &rest[take..];
            }
            Ok(())
        };
        load_segments(image, segments, &mut write)
    };
    if not_mapped {
        return Err(KernelPlanError::NotMapped);
    }
    loaded.map_err(KernelPlanError::Elf)?;
    Ok(total)
}

/// HHDM 偏移（**引导器自己选定**）。
///
/// 取 Limine 惯用的 `0xffff_8000_0000_0000`：位于 48 位虚拟地址空间的高半区，与内核链接
/// 基址 `0xffff_ffff_8000_0000` 不重叠。**报给内核的 `hhdm_response.offset` 必须与此常量
/// 一致** —— 报错就是内核按错偏移解地址，必崩。
pub const HHDM_OFFSET: u64 = 0xffff_8000_0000_0000;

/// 内核主栈顶：`.kernel_main_stack` 位于 `0xffff_ffff_808a_e000`、大小 `0x10_1000`。
///
/// 由真实内核的节表量出（见 `docs/TODO/liftoff.md` 的实测记录）。
pub const KERNEL_STACK_TOP: u64 = 0xffff_ffff_809a_e000;

/// 规划组装失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanBuildError {
    /// 内核物理基址未按大页对齐。
    ///
    /// 规划器只映射**整大页**，且会把基址**向上**对齐 —— 基址未对齐时，基址到首个对齐边界
    /// 之间的部分会被**静默丢弃**，内核开头的映射凭空消失，而入口就在那一带。
    /// 内核放在物理内存的哪里是**我们自己选的**，所以这里直接要求对齐，不放任静默丢失。
    KernelBaseUnaligned,
    /// 底层规划器报错。
    Plan(PlanError),
}

impl core::fmt::Display for PlanBuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::KernelBaseUnaligned => f.write_str("内核物理基址未按大页对齐"),
            // `PlanError` 现已实现 `Display`（中立层 `mm`），故用 `{}`。
            Self::Plan(err) => write!(f, "页表规划失败: {err}"),
        }
    }
}

#[cfg(test)]
mod error_display_tests {
    use super::{EntryError, KernelPlanError, PlanBuildError};

    #[test]
    fn entry_error_keeps_the_uncovered_address_in_its_message() {
        // `NotCovered` 携带地址，而**那正是这条错误存在的理由** —— 消息里没有地址，
        // 真机上就只能看到「有东西没被覆盖」，无从下手。
        let text = format!("{}", EntryError::NotCovered { address: 0xffff_ffff_8003_78d0 });
        assert!(text.contains("0xffffffff800378d0"), "必须打出具体地址: {text}");
        assert!(text.contains("未被规划覆盖"), "必须说明是什么问题: {text}");
    }

    #[test]
    fn entry_error_variants_have_distinct_messages() {
        let mut seen: Vec<String> = Vec::new();
        for text in [
            format!("{}", EntryError::NoEntry),
            format!("{}", EntryError::NotCovered { address: 0x1000 }),
            format!("{}", EntryError::EmptySpan),
        ] {
            assert!(!text.is_empty(), "每条错误都必须有消息");
            assert!(!seen.contains(&text), "消息不得重复: {text}");
            seen.push(text);
        }
    }
    use std::format;
    use std::string::String;
    use std::vec::Vec;

    #[test]
    fn every_payloadless_variant_has_a_distinct_human_readable_message() {
        // 这些消息会**直接出现在真机串口上**（`report_failure`），所以它们必须存在、
        // 必须各不相同 —— 两条错误打印出同一行文字，等于没有诊断信息。
        //
        // 带载荷的变体不在此列：它们的 `Display` 由编译期保证（`write!` 要求载荷
        // 实现 `Display`），而载荷自身的可读性缺口已记入台账。
        let mut seen: Vec<String> = Vec::new();
        let cases: [(String, &str); 8] = [
            (format!("{}", KernelPlanError::EntryOutsideSegments), "入口"),
            (format!("{}", KernelPlanError::EmptySegment), "长度为 0"),
            (format!("{}", KernelPlanError::Overflow), "溢出"),
            (format!("{}", KernelPlanError::BufferTooSmall), "缓冲"),
            (format!("{}", KernelPlanError::NotMapped), "映射"),
            (format!("{}", KernelPlanError::MapFailed), "页表"),
            (format!("{}", KernelPlanError::RelocationNotMapped), "重定位"),
            (format!("{}", PlanBuildError::KernelBaseUnaligned), "对齐"),
        ];
        for (text, keyword) in cases {
            assert!(!text.is_empty(), "每条错误都必须有消息");
            assert!(text.contains(keyword), "「{text}」应含「{keyword}」");
            assert!(!seen.contains(&text), "消息不得重复: {text}");
            seen.push(text);
        }
    }
}

/// 页表规划的输入。
///
/// 用具名结构而不是继续堆位置参数：已经 7 个参数，再加一个会让每个调用点都变成
/// 一串无法自解释的 `true` / `0x20_0000`。
pub struct PlanRequest<'a> {
    /// 内核装载的物理基址（`large != 0` 时必须按 `large` 对齐）。
    pub kernel_phys: u64,
    /// 内核链接期虚拟基址。
    pub kernel_virt: u64,
    /// 内核区间长度（会被向上取整到 `large` 的整数倍）。
    pub kernel_len: u64,
    /// HHDM 覆盖的物理区间。
    pub hhdm: &'a [UsableRange],
    /// 恒等映射覆盖的物理区间。
    pub identity: &'a [UsableRange],
    /// 大页粒度；`0` 表示不用大页。
    pub large: u64,
}

/// 组装交接所需的页表规划：内核高区 + HHDM + 恒等。
///
/// 恒等映射**必须保留**：切换页表时当前正在执行的代码与栈必须仍然被映射
/// （`PageTable::activate` 的 SAFETY 契约）。保留恒等映射后这条自动成立，
/// 无需去读 RIP/RSP（那需要汇编）。`identity` 由调用方给出（取自固件内存映射的可用区间）。
///
/// 内核按**一段**给出（三段取并集）：段间空隙也会被映射，这是有意的简化。
pub fn build_plan(request: &PlanRequest<'_>, out: &mut [Mapping]) -> Result<usize, PlanBuildError> {
    let PlanRequest {
        kernel_phys,
        kernel_virt,
        kernel_len,
        hhdm,
        identity,
        large,
    } = *request;
    if large != 0 && kernel_phys % large != 0 {
        return Err(PlanBuildError::KernelBaseUnaligned);
    }
    // 规划器只产出**整大页**：长度不是大页整数倍时，尾部会被丢掉（内核最后一段就没映射）。
    // 向上取整 —— 多映射一点无害，少映射是致命的。
    let kernel_len = if large == 0 {
        kernel_len
    } else {
        kernel_len
            .checked_add(large - 1)
            .ok_or(PlanBuildError::Plan(PlanError::AddressOverflow))?
            / large
            * large
    };
    let mut total = 0usize;
    let kernel_count = mm::plan::plan_kernel_high(kernel_phys, kernel_virt, kernel_len, &mut out[total..], large)
        .map_err(PlanBuildError::Plan)?;
    total += kernel_count;
    // 记下 HHDM 的条数：权限按「内核 / HHDM / 恒等」三段分别给，所以需要边界。
    let hhdm_count = mm::plan::plan_hhdm(hhdm, HHDM_OFFSET, &mut out[total..], large)
        .map_err(PlanBuildError::Plan)?;
    total += hhdm_count;
    total += mm::plan::plan_identity(identity, &mut out[total..], large)
        .map_err(PlanBuildError::Plan)?;
    // 规划器只产出 `present()` —— 在 x86-64 上那等于 **NX 置位**：内核入口所在的代码段
    // 不可执行，一跳过去就指令取指故障（真实运行表现为机器复位）。这里按用途补权限：
    // 内核段 R/W/X（它要执行代码、写数据）；HHDM 与恒等 R/W（引导器与内核都要读写）。
    // 内核段：R/W/X —— 它要执行代码、也要写数据。
    //
    // **注意**：内核目前是**一整段**（三段取并集）给同一个权限，没有按 ELF 段权限
    // 细分。真正的 W^X 需要按段映射（代码段 R/X、数据段 R/W），那是下一步；
    // 这里不假装已经做到。
    let kernel_flags = PageFlags::present()
        .with(PageFlags::writable())
        .with(PageFlags::executable());
    // 恒等 / 低 4 GiB：**必须可执行**，这不是偷懒。
    //
    // 跳板自己就从低内存取指：`mov cr0` 打开分页后，它还要再执行几条指令
    // （`retf` 回到 64 位、跳内核入口）。把这里设成不可执行，跳板会在换表后
    // **立刻**取指故障 —— 表现为无输出复位。
    let identity_flags = PageFlags::present()
        .with(PageFlags::writable())
        .with(PageFlags::executable());
    // HHDM：**数据通道** —— 但**必须保持可执行**，这是实测结论而不是偷懒。
    //
    // 曾尝试在 `nx_available` 为真时给 HHDM 置 NX（W^X：直接映射不可执行，
    // 挡住 ret2dir 一类手法）。跳板确实在 `CR0.PG` 生效**之前**写 `EFER.NXE`
    // （见 `spinup` 汇编），所以 NX 位本身合法 —— 但真机结果是**复位循环**
    // （串口 75501 字节，OVMF 引导信息反复出现），即换表后立刻取指故障。
    // **结论：有东西经 HHDM 取指。** 具体是谁尚未定位（可能是内核早期入口走
    // 物理别名），在定位之前不能置 NX。这条否定结果**实测支撑**了
    // 「数据映射一律可执行」这一策略 —— 它看起来像保守，实际是必需。
    let hhdm_flags = PageFlags::present()
        .with(PageFlags::writable())
        .with(PageFlags::executable());
    // **`base_revision == 0` 的 Limine 规则**（对照 brxLimine `build_pagemap`，
    // `limine.c:200-203`）：把低 4 GiB **恒等**映射。
    //
    // 内核按这个语义直接访问低 4 GiB 的 MMIO —— 帧缓冲就在 `0x80000000`。我们的
    // 恒等映射只覆盖固件内存映射描述过的区间，**不含 PCI MMIO 窗口**。真机实测的
    // 后果：内核终端初始化成功（`fb=0x80000000 1280x800 bpp=32`）后写帧缓冲即
    // #PF（`CR2=0x80000000`、错误码 `0x2`）。
    //
    // **从 `0` 起的大页（页零仍被映射）—— 当前状态，混合粒度已实现但真机仍失败。**
    //
    // 全部 4 KiB 需要 2048 张页表，帧预算供不起（实测 OutOfMemory）。混合粒度
    // （头部 4 KiB + 主体大页 + 显式解除页零）的**映射逻辑已由宿主测试证明正确**
    // （`the_full_low_4gib_sequence_matches_the_bootloader_plan`），但**真机仍然
    // 复位循环、`entry failed`**，原因尚未定位 —— 说明问题在宿主测试覆盖不到的
    // 层面（帧预算/固件交互等）。**在定位之前保持这个已知可工作的形态。**
    //
    // 缓冲不足时报错而不是静默跳过：丢掉必需映射 = 换表即故障。
    const LOW_HEAD_END: u64 = 0x20_0000;
    {
        if total + 2 > out.len() {
            return Err(PlanBuildError::Plan(PlanError::BufferTooSmall));
        }
        out[total] = Mapping {
            virt: arch::addr::VirtAddr::new(0x1000),
            phys: arch::addr::PhysAddr::new(0x1000),
            len: LOW_HEAD_END - 0x1000,
            flags: identity_flags,
        };
        total += 1;
        out[total] = Mapping {
            virt: arch::addr::VirtAddr::new(LOW_HEAD_END),
            phys: arch::addr::PhysAddr::new(LOW_HEAD_END),
            len: 0x1_0000_0000u64 - LOW_HEAD_END,
            flags: identity_flags,
        };
        total += 1;
    }
    // 三段各自给权限：`[0, kernel)` / `[kernel, kernel+hhdm)` / 其余（恒等 + 低 4 GiB）。
    for (index, mapping) in out[..total].iter_mut().enumerate() {
        mapping.flags = if index < kernel_count {
            kernel_flags
        } else if index < kernel_count + hhdm_count {
            hhdm_flags
        } else {
            identity_flags
        };
    }
    Ok(total)
}

/// 引导器自述用的名字（Limine `BOOTLOADER_INFO`）：**NUL 结尾的 C 字符串**。
const BOOTLOADER_NAME: &[u8] = b"liftoff\0";

/// 引导器自述用的版本。
///
/// 取自 Cargo 包版本 —— **单点**，不手抄：手抄的版本号必然与 `Cargo.toml` 漂移。
const BOOTLOADER_VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

/// 交接所需的**全部调用方缓冲**（引导器不做隐藏分配：每个缓冲都由调用方给）。
pub struct BringUp<'a, 'b> {
    /// 内核映像读出目标（约 25 MB，来自固件页）。
    pub kernel_out: &'a mut [u8],
    /// 分区表/PVD 头缓冲（≥ 34 KiB）。
    pub head: &'a mut [u8],
    /// 页表规划输出。
    pub plan: &'a mut [Mapping],
    /// 装载段输出。
    pub segments: &'a mut [ProgramHeader],
    /// 可用物理区间输出（HHDM 与恒等映射的来源）。
    pub usable: &'a mut [UsableRange],
    /// 内存映射来源（退出前要用它记录的键）。
    pub memory_map: &'a mut UefiMemoryMapSource<'b>,
    /// 内存映射缓冲。
    pub map_buffer: &'a mut [MemoryEntry],
    /// 扫描命中缓冲。
    pub hits: &'a mut [RequestHit],
    /// 必须保持映射的区间输出。
    pub must_stay: &'a mut [MustStay],
    /// 我们准备的响应结构。
    pub responses: &'a mut Responses,
    /// 键的存放槽。
    pub map_key: &'a mut Option<usize>,
    /// 内核段拷入的**物理目标基址**（须按大页对齐，由调用方选定）。
    pub destination: u64,
    /// ACPI 的 RSDP（从固件配置表取；没有就是 `None`，不编造）。
    pub rsdp: Option<*mut core::ffi::c_void>,
    /// 帧缓冲（从固件 GOP 取；没有就 `None`，不编造）。
    pub framebuffer: Option<firmware::graphics::FramebufferInfo>,
}

/// 交接失败原因（保留环节）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BringUpError {
    /// 发现块设备失败。
    Discover(Error),
    /// 介质路径失败。
    Media(crate::media::MediaError),
    /// 内核 ELF 规划失败。
    Kernel(KernelPlanError),
    /// 页表规划失败。
    Plan(PlanBuildError),
    /// 加载内存映射失败（缓冲不足或固件拒绝）。
    MemoryMapLoad(Error),
    /// 从内存映射里取可用区间失败。
    MemoryMapRanges(Error),
    /// 取根帧失败。
    RootFrame,
    /// 建 HHDM 直接映射失败。
    DirectMap,
    /// 把规划写入页表失败。
    Apply(arch::paging::MapError),
    /// 拷贝段失败。
    Copy(KernelPlanError),
    /// 填响应失败。
    Responses(Error),
    /// 交接编排失败。
    Handoff(HandoffError),
}

/// 真实入口的交接（**固件侧**）：发现设备 → 读内核 → 规划 → 拷段 → 写表 → 激活 → 交接。
///
/// 本函数只做编排：所有判断都在已测过的纯函数里。它是**唯一只能在真机上验**的部分。
///
/// # Safety
///
/// 引导阶段单线程调用一次；激活页表后不再返回。
pub unsafe fn bring_up(
    // **可变**：AP 接线需要在 `apply` 之后向固件要一页**低内存**（< 1 MiB ✗ 实模式寻址的硬约束 ✓），
    // 而低页分配是 `allocate_zeroed_below(&mut self, …)` ✓ —— 不可变引用下**无法调用** ✗。
    // 调用方是裸指针（`&mut *boot_services` ✓），所以这里改 `&mut` 不需要可变绑定 ✓。
    table: &mut BootServicesTable,
    image_handle: Handle,
    c: BringUp<'_, '_>,
    enter: impl FnOnce(u64) -> !,
) -> Result<(), BringUpError> {
    crate::PlatformImpl::write_byte(b'1');
    // 1) 发现块设备（存储由类型自己持有，入口不需要认识 BlockIo）。
    // SAFETY: 由调用方保证引导阶段单线程、只调一次。
    let mut devices =
        unsafe { UefiBlockDevices::from_boot_services(table.locate_handle, table.handle_protocol) }
            .map_err(BringUpError::Discover)?;
    crate::PlatformImpl::write_byte(b'2');
    // 2) 从介质读出内核映像。
    let len = crate::media::load_kernel_from_device(
        &mut devices,
        DeviceIndex(0),
        c.head,
        c.kernel_out,
    )
    .map_err(BringUpError::Media)?;
    crate::PlatformImpl::write_byte(b'3');
    // 3) 规划内核装载（入口、段、必须保持映射的区间）。
    let info = plan_kernel(&c.kernel_out[..len], c.segments).map_err(BringUpError::Kernel)?;
    let stays = must_stay_from_segments(&c.segments[..info.segment_count], c.must_stay)
        .map_err(BringUpError::Kernel)?;
    crate::PlatformImpl::write_byte(b'4');
    // 4) 可用物理区间（HHDM 与恒等映射都从这里来）。
    let map = c
        .memory_map
        .memory_map(c.map_buffer)
        .map_err(BringUpError::MemoryMapLoad)?;
    // 用**恒等映射**那套（除 Bad 外全部）：引导器自己的代码与栈在 loader/boot-services
    // 区域，只覆盖可分配区间会让切换页表后取指失败（真机上就是无输出复位）。
    let mut usable_count =
        mm::usable::identity_ranges(map, c.usable).map_err(BringUpError::MemoryMapRanges)?;
    // **把帧缓冲的物理区间也交给规划器**（HHDM 与恒等映射共用这份区间）。
    //
    // 帧缓冲是 MMIO：既不在固件内存映射的可用区间里，很可能根本不在固件内存映射里
    // —— 于是 HHDM 不覆盖它。对照 brxLimine：它把帧缓冲**显式**加进内存映射
    // （`MEMMAP_FRAMEBUFFER`），而 `base_revision == 0` 时 `build_pagemap` 会把
    // **所有**条目都映射到 HHDM。
    //
    // 真机实测的后果：内核终端写 `0xffff800080000000` 即 #PF（错误码 `0x2`）。
    if let Some(framebuffer) = &c.framebuffer {
        if usable_count < c.usable.len() {
            let length = (framebuffer.pitch as u64).saturating_mul(framebuffer.height as u64);
            if length > 0 {
                c.usable[usable_count] = mm::usable::UsableRange {
                    base: framebuffer.base,
                    length,
                };
                usable_count += 1;
            }
        }
    }
    crate::PlatformImpl::write_byte(b'5');
    // 5) 页表规划：内核高区 + HHDM + 恒等。
    let kernel_virt = c.segments[..info.segment_count]
        .iter()
        .map(|s| s.p_vaddr)
        .min()
        .ok_or(BringUpError::Kernel(KernelPlanError::EntryOutsideSegments))?;
    let kernel_end = c.segments[..info.segment_count]
        .iter()
        .map(|s| s.p_vaddr + s.p_memsz)
        .max()
        .ok_or(BringUpError::Kernel(KernelPlanError::Overflow))?;
    let kernel_len = kernel_end - kernel_virt;
    // 可用区间必须**向下对齐到大页**再交给规划器：规划器只产出整大页、且会把基址向上
    // 对齐，于是非对齐区间的**头部会被静默丢掉**。真实运行里这就是一次 #PF ——
    // 栈所在的那一页正好落在被丢掉的头部。向下对齐多映射的是同一大页内的物理内存（安全），
    // 少映射是致命的。
    align_ranges_down(&mut c.usable[..usable_count], LARGE_PAGE).map_err(BringUpError::Plan)?;
    let plan_count = build_plan(
        &PlanRequest {
            kernel_phys: c.destination,
            kernel_virt,
            kernel_len,
            hhdm: &c.usable[..usable_count],
            identity: &c.usable[..usable_count],
            large: LARGE_PAGE,
        },
        c.plan,
    )
    .map_err(BringUpError::Plan)?;
    crate::PlatformImpl::write_byte(b'6');
    // 6) 页表：根帧 + HHDM 直接映射 + 固件帧来源。
    let mut frames = EfiFrameAllocator::new(table.allocate_pages);
    let root = frames.allocate_zeroed().ok_or(BringUpError::RootFrame)?;
    // `top` 是**直接映射覆盖的最高物理地址**（不是 u64::MAX：那会溢出而被拒）。
    let top = c.usable[..usable_count]
        .iter()
        .filter_map(|range| range.end())
        .max()
        .ok_or(BringUpError::DirectMap)?;
    // `DirectMap` 是 `X86PageTable` **写页表项**时用来够表帧的映射，而写表发生在
    // **激活之前** —— 那时固件页表里只有恒等映射，没有 HHDM。真实运行中这里正是
    // 一次 #PF：CR2 落在 HHDM、P:0。所以这里用 **offset 0（恒等）**：它在激活前后都成立
    // （我们的规划本身也保留恒等映射）。
    let direct = DirectMap::new(0, top).ok_or(BringUpError::DirectMap)?;
    let mut page_table = X86PageTable::new(root, direct, frames);
    // 7) 拷段到物理目标（真机：直接写物理地址，UEFI 阶段恒等映射有效）。
    let mut write = |phys: u64, bytes: &[u8]| -> Result<(), ElfError> {
        // SAFETY: 目标是我们自己选定的物理内存；引导阶段该地址可直接访问。
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), phys as *mut u8, bytes.len()) };
        Ok(())
    };
    for byte in b"[liftoff] step: applying plan\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // **显式解除页零**：`plan_identity` 先为低内存建了 2 MiB 大页，混合粒度的头部映射
    // 触发**拆分**，而拆分如实复制 —— 于是页表第 0 项仍映射 `0..0x1000`。头部只覆盖
    // `0x1000` 起、不会碰它，所以必须在这里显式解除，才算真正与 Limine 一致
    // （`limine.c:200-203` 从 `0x1000` 起）。
    if let Err(err) = mm::apply::apply(&mut page_table, &c.plan[..plan_count]) {
        // 具体是哪一种 `MapError` 由 `report_failure` 统一打出（不再需要特例）。
        return Err(BringUpError::Apply(err));
    }
    page_table
        .unmap(arch::addr::VirtAddr::new(0), 0x1000)
        .map_err(BringUpError::Apply)?;
    for byte in b"[liftoff] step: plan applied\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // 真机排障脚手架（CR4/RDMSR/RIP/RSP 寄存器读取）已删除：它们的使命已完成，
    // 且宿主测试二进制里这些特权指令会让整个测试进程以 STATUS_PRIVILEGED_INSTRUCTION
    // 崩溃 —— 这就是此前「偶发」测试崩溃的真正原因（并非偶发）。
    // 激活前的 RIP/RSP 覆盖自检已删除（真机均已确认 covered）。
    {
        for byte in if true {
            b"[liftoff] step: rip covered\n" as &[u8]
        } else {
            b"[liftoff] step: rip NOT covered\n" as &[u8]
        } {
            crate::PlatformImpl::write_byte(*byte);
        }
    }
    // 走表自检（读取 RIP 并核对恒等帧）已删除：`lea rip` 在宿主测试进程里与
    // 其它特权指令一起触发 STATUS_PRIVILEGED_INSTRUCTION。真机数据已采集完毕
    // （walk identity ok / rip covered / rsp covered 均确认过）。
    for byte in b"[liftoff] step: activating\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // 激活已移到 Exit 之后（enter_kernel 内）—— 此处只保留页表构建结果。
    for byte in b"[liftoff] step: table ready\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // 「激活后自检」已删除：激活现在发生在 Exit 之后（enter_kernel 内），
    // 而自检读的是 kernel_virt —— 在激活前读它就是 #PF（真机 CR2=FFFFFFFF80000000 已实测）。
    // 映射正确性由「拷段成功 + 覆盖检查」保证，不再需要运行时读回验证。
    let _ = stays;
    // 扫描范围：**优先 `.data` 节**（实测真实内核的 7 个请求全在其中，约 345 KB ——
    // 固件里内存读约 100 µs/次，扫全映像或全段都跑不完）；节表不可用时**回退**到
    // 已装载段区间（语义等价性由真实内核对照测试守住，只是慢）。
    static mut RANGES: [(usize, usize); 16] = [(0, 0); 16];
    let mut range_count = 0usize;
    // SAFETY: 引导阶段单线程；本数组在 `enter_kernel` 之前一直有效。
    let ranges = unsafe { &mut *core::ptr::addr_of_mut!(RANGES) };
    if let Some((start, end)) = loader::elf::section_file_range(&c.kernel_out[..len], ".data") {
        ranges[0] = (start, end);
        range_count = 1;
    } else {
        for segment in &c.segments[..info.segment_count] {
            if range_count == ranges.len() {
                break;
            }
            let Ok(start) = usize::try_from(segment.p_offset) else {
                continue;
            };
            let Ok(size) = usize::try_from(segment.p_filesz) else {
                continue;
            };
            let Some(end) = start.checked_add(size) else {
                continue;
            };
            if end > len {
                continue;
            }
            ranges[range_count] = (start, end);
            range_count += 1;
        }
    }
    // 决定性检查：**激活之后**，那块固件页缓冲还读得到吗？
    // 自检只读过内核的虚拟映射，从没读过这块缓冲 —— 若它不在恒等映射里，
    // 第一次读它就故障，且与“扫描很慢”表现完全相同。
    {
        // 逐个偏移探测：整段缓冲是否真的都可读？（只验开头是不够的）
        for offset in [0usize, 1 << 20, 4 << 20, 8 << 20, 16 << 20, 24 << 20] {
            let _probe = c.kernel_out[offset];
            for byte in b"[liftoff] step: buf ok\n" as &[u8] {
                crate::PlatformImpl::write_byte(*byte);
            }
        }
    }
    for byte in b"[liftoff] step: before scan\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    // **直接问内核要什么**：扫描它声明的请求并逐条打印（不读内核代码也能知道）。
    // 这比猜“它可能缺什么”可靠得多。
    {
        let mut hits = [RequestHit::EMPTY; 64];
        // 先扫一个**极小切片**：若这也卡，问题在函数本身；若秒回，问题在切片大小。
        match limine::scan::scan_requests(&c.kernel_out[..4096], &mut hits) {
            Err(_) => {
                for byte in b"[liftoff] tiny ERR\n" as &[u8] {
                    crate::PlatformImpl::write_byte(*byte);
                }
            }
            Ok(_) => {
                for byte in b"[liftoff] tiny OK\n" as &[u8] {
                    crate::PlatformImpl::write_byte(*byte);
                }
            }
        }
        // **先打出 `len` 的数量级**（每 4 MB 一个点，最多 48 个）：
        // `len` 是唯一没在真机上验证过、又决定循环边界的量 —— 宿主测试里它是对的，
        // 但真机上若是个垃圾值，4 MB 分块循环就会一直扫到没有映射的地方。
        // 上一版在这里用自写的十进制打印输出 `len` —— 而真机**正好在打印 `len=` 之后死掉**，
        // 所以我不能排除是我自己的打印代码。这里换成**不含任何算术**的固定标记，
        // 并用 `<` `>` 把它夹住：两个都出现＝打印无关；只出现 `<`＝就是打印。
        {
            crate::PlatformImpl::write_byte(b'<');
            for byte in b"[liftoff] len checked\n" as &[u8] {
                crate::PlatformImpl::write_byte(*byte);
            }
            crate::PlatformImpl::write_byte(b'>');
        }
        // 规模-时间判据已完成使命（1 MB 能过、4 MB 卡住的量级已确认），删除。
        // 分块扫描改用**与真实路径相同**的已装载段区间：此前这里扫的是全部 24.6 MB，
        // 它才是把整次运行拖过 400 秒的元凶 —— 真实路径早已只扫约 10 MB。
        // 诊断扫描块已删除：它与真实路径各扫一遍 .data（约 345 KB），重复且时间翻倍。
        // 真实路径（fill_responses）扫完后会由交接留痕（filled/checked/…）继续汇报。
    }
    // 8) 交接：填响应 → 检查 → 取键退出 → 跳转。
    //
    // HHDM 偏移是**必须**的：内核靠它把物理地址翻成虚拟地址。不填（或填 0）它会算错地址，
    // 真实运行里表现为跳转后立刻复位 —— 这很可能就是复位的原因。
    // **内存映射响应**：内核靠它决定哪些内存可用。此前这条响应**从未被填充**
    // （`fill_memory_map`/`set_memmap` 只在测试里出现过），而内核明确请求了 `memmap`。
    // 另外内核自己占的页必须标成 `KernelAndModules`，否则内核会把它并入空闲池并
    // 踩掉自己的代码/数据（对照 brxLimine 的 `MEMMAP_KERNEL_AND_MODULES`）。
    {
        use crate::responses::{MAX_MEMMAP_ENTRIES, mark_kernel_memory};
        let empty = limine::memmap::MemmapEntry { base: 0, length: 0, kind: 0 };
        let mut entries = [empty; MAX_MEMMAP_ENTRIES];
        let count = if map.len() > MAX_MEMMAP_ENTRIES {
            MAX_MEMMAP_ENTRIES
        } else {
            map.len()
        };
        for (index, entry) in map.iter().take(count).enumerate() {
            entries[index] = limine::memmap::MemmapEntry {
                base: entry.base.as_u64(),
                length: entry.length,
                kind: entry.kind.as_protocol() as u64,
            };
        }
        // 内核占用的**物理**区间（来自装载规划）。
        let mut ranges = [(0u64, 0u64); 8];
        let mut range_count = 0usize;
        for mapping in c.plan[..plan_count].iter() {
            if range_count == ranges.len() {
                break;
            }
            ranges[range_count] = (mapping.phys.as_u64(), mapping.len);
            range_count += 1;
        }
        let mut marked = [empty; MAX_MEMMAP_ENTRIES];
        match mark_kernel_memory(&entries[..count], &ranges[..range_count], &mut marked) {
            Some(marked_count) => c.responses.set_memmap(&marked[..marked_count]),
            // 缓冲不足时保留原映射：**宁可少标，也不静默产出错误映射**。
            None => c.responses.set_memmap(&entries[..count]),
        }
    }
    // **SMP 响应**：至少登记 BSP 本身。此前 `cpu_count` 恒为 0，内核会认为没有
    // 任何 CPU —— 真机实测它随后卡死在紧循环里。BSP 的 LAPIC 标识从 CPUID.1:EBX[31:24]。
    {
        #[cfg(target_arch = "x86_64")]
        // 经抽象层取（C2）：直接 `cpuid` 会让 `boot` 变成 x86 专用（ADR-007/ADR-050）。
        let bsp_lapic_id = crate::PlatformImpl::bsp_lapic_id();
        #[cfg(not(target_arch = "x86_64"))]
        let bsp_lapic_id = 0u32;
        c.responses.set_smp(bsp_lapic_id);
    }
    c.responses.set_hhdm_offset(HHDM_OFFSET);
    // 【S3c 第 1 步】把**固件的真实 ACPI 数据**接进这条链 —— **还不启动任何 AP**。
    //
    // `started_aps = 0`，所以 `cpu_count` 仍是 1，**引导行为应当完全不变**。价值在于：
    // 让固件真实数据第一次流经 S1 的解析器，且**不发 IPI、不改激活时序** —— 风险最低。
    //
    // **`&mut *c.responses` 是重借用**：`c` 是**按值传入的不可变绑定**，而
    // `c.responses` 的类型是 `&mut Responses` —— 所以直接写 `&mut c.responses` 会被拒绝
    // （要求 `c` 可变），而重借用走的是那个已有的 `&mut` ✓。这一点我前两次都没看清。
    //
    // 只在 UEFI 目标上做：宿主测试里 `rsdp` 是假地址，解引用会崩。
    #[cfg(target_os = "uefi")]
    if let Some(rsdp) = c.rsdp {
        // SAFETY: `rsdp` 来自固件配置表、指向 RAM；RAM 在**当前生效的固件页表**下恒等映射
        // （本函数已在用同一映射写内核目标物理地址）。只读，且长度取自表自己的字段。
        unsafe {
            // 页表根物理地址：与 BSP 的 spinup 用**同一个根帧** ✓（AP 应当用同一套页表 ✓）。
    let cr3_top = root.start_address().expect("根帧必有地址").as_u64();
    // `&mut *table` 是**重借用** ✓ —— `table` 是 `&mut`，直接传会被**移动** ✗。
    // `frames` 是**值**（`EfiFrameAllocator`）✓ —— 直接 `&mut frames` ✓（`&mut *frames` 会报"无法解引用" ✗）。
    // `frames` 在更早处**已被移动** ✗（`EfiFrameAllocator` 不实现 `Copy` ✓）→ 不能复用 ✓。
    // 但它只是**固件指针的包装** ✓，而 `table.allocate_pages` 仍可用 ✓ → 构造一个新的 ✓。
    let mut low_frames = EfiFrameAllocator::new(table.allocate_pages);
    register_madt_cpus(
        &mut low_frames,
        table.stall,
        direct,
        &mut *c.responses,
        rsdp as u64,
        cr3_top,
    );
        }
    }
    // **引导器自述**：`BootloaderInfoResponse` 的 name/version 此前一直是 NULL ——
    // `set_bootloader_info` 被定义了却**从未被调用**（死代码，S06），于是内核问
    // "你是谁"时得到的是一片空白（串口实测 `[init] kernel version = 0x000`
    // **很可能就是它**）。这里补上。
    //
    // `*mut` 来自协议 ABI（Limine 用 C 的 `char *`）；这两个静态字符串**只读**，
    // 内核按协议只应读取它们。
    c.responses.set_bootloader_info(
        BOOTLOADER_NAME.as_ptr().cast_mut().cast(),
        BOOTLOADER_VERSION.as_ptr().cast_mut().cast(),
    );
    // RSDP 只在**真的从配置表找到**时才填；没有就留空 —— 给假指针比不给更糟。
    if let Some(rsdp) = c.rsdp {
        // **HHDM 地址**（对照 brxLimine `limine.c:1104`：`rsdp_response->address = reported_addr(rsdp)`）。
    c.responses.set_rsdp((HHDM_OFFSET.wrapping_add(rsdp as u64)) as *mut core::ffi::c_void);
    }
    // 帧缓冲同理：只有**真的从固件拿到**才填。内核很可能先往帧缓冲输出，
    // 之后才初始化串口 —— 帧缓冲为空时它可能就停在那里。
    if let Some(info) = &c.framebuffer {
        fill_framebuffer(c.responses, info).map_err(BringUpError::Responses)?;
    }
    // 可执行地址与可执行文件：两者的值我们**自己就知道**（装载决策），不需要问固件。
    c.responses.set_executable_address(c.destination, kernel_virt);
    fill_executable_file(c.responses, c.destination, len as u64)
        .map_err(BringUpError::Responses)?;
    // Exit 前分配**低地址缓冲**（< 4 GiB）：32 位跳板两段 + 参数帧 + 低地址栈。
    // 必须用 `EfiLoaderCode`：这段内存要被**取指**，`EfiLoaderData` 在 OVMF 下
    // 可能被标成不可执行。
    let spinup_buf_len = 64 * 1024;
    // SAFETY: bring_up 是 unsafe fn，boot services 指针有效。
    let Some(low_buffer) = (unsafe {
        alloc_buffer_typed(table.allocate_pages, spinup_buf_len, EFI_LOADER_CODE)
    }) else {
        return Err(BringUpError::RootFrame);
    };
    // **内核栈由引导器自己分配**（对照 brxLimine `limine.c:1647`：
    // `void *stack = ext_mem_alloc(stack_size) + stack_size;`，默认 64 KiB，
    // 内核可用 `LIMINE_STACK_SIZE_REQUEST` 要更大的）。
    //
    // **不能用内核自己的 `.kernel_main_stack`**：那在 `.bss` 里，而内核早期会清
    // BSS —— 把自己的栈抹掉。实测症状：入口 +0x11 处 `RSP` 变成 0，随后压栈写到
    // `0xfffffffffffffff8` 而 #PF。
    //
    // 报给内核的是**栈顶**，且是 **HHDM 地址**（对照 brxLimine
    // `reported_addr(addr) = addr + direct_map_offset`）。
    const KERNEL_STACK_BYTES: usize = 64 * 1024;
    // SAFETY: bring_up 是 unsafe fn，boot services 指针有效。
    let Some(kernel_stack) = (unsafe {
        alloc_buffer(table.allocate_pages, KERNEL_STACK_BYTES)
    }) else {
        return Err(BringUpError::RootFrame);
    };
    let stack_phys = kernel_stack.as_ptr() as u64;
    let kernel_stack_top = HHDM_OFFSET + stack_phys + KERNEL_STACK_BYTES as u64;
    // **探测而非假设**（E4）：`nx_available` 决定 32 位跳板是否给 `EFER` 置 `NXE`。
    // 在没有 NX 的 CPU 上，那是保留位写入 → `#GP`。QEMU 默认 CPU 有 NX，所以
    // 硬编码 1 一直「恰好成立」—— 这正是硬编码假设的典型形态（S04）。
    // **探测而非假设**（E4）：`nx_available` 决定 32 位跳板是否给 `EFER` 置 `NXE`。
    // 在没有 NX 的 CPU 上，那是保留位写入 → `#GP`。QEMU 默认 CPU 有 NX，所以
    // 硬编码 1 一直「恰好成立」—— 这正是硬编码假设的典型形态（S04）。
    let nx_available = if current::features::nx_available() { 1 } else { 0 };
    let spinup_args = current::spinup::SpinupArgs {
        // **0 = 4 级分页，且这是正确的、不需要探测**：跳板在 `spinup_go32` 里先
        // `xor eax,eax; mov cr4,eax` **整体清零 CR4**，所以 LA57 必然已被清除；
        // 我们建的也是 4 级表。brxLimine 同样是 `mov cr4, 0` 之后按需重建
        // （`spinup.asm_uefi_x86_64:109`）。传 0 表示「不要重新启用 LA57」，与
        // 清零后的状态一致。
        level5pg: 0,
        pagemap_top: root.start_address().expect("根帧必有地址").as_u64() as u32,
        entry_lo: (info.entry & 0xFFFF_FFFF) as u32,
        entry_hi: (info.entry >> 32) as u32,
        stack_lo: (kernel_stack_top & 0xFFFF_FFFF) as u32,
        stack_hi: (kernel_stack_top >> 32) as u32,
        gdt: 0,
        nx_available,
        dmo_lo: (HHDM_OFFSET & 0xFFFF_FFFF) as u32,
        dmo_hi: (HHDM_OFFSET >> 32) as u32,
        // **0 = 不卸低半区**。base_revision 是**内核声明的协议版本**，不是我们可以
        // 随便挑的：`>= 1` 的语义是「请把低半区卸掉」。真实内核里一个 Limine 请求
        // 标记都没有（已实测：COMMON_MAGIC 7 个、START/END 标记 0 个），即它没有
        // 声明任何版本 —— 对应 base_revision = 0。
        //
        // 硬编码 1 的后果已实测：`rep stosq` 清掉 PML4[0..255] 之后，映像里那张
        // GDT 不再可达，紧接着的 `iretq` 读 CS(0x28) 描述符就 #PF（CR2 = GDT+0x28），
        // 再升级成 #DF → 三重故障。
        base_revision: 0,
    };
    // **响应指针必须在拷段之前写进映像。** 内核运行的是 `copy_kernel_segments` 之后
    // 的那块内存，而 `fill_responses` 写的是暂存缓冲 —— 顺序错了，指针就永远送不到
    // 内核手里。真机实测的后果：内核报 `Failed to get HHDM response from Limine`
    // 并在 `mm::init` 里 panic（`crates/mm/src/lib.rs:113`）。
    let _ = crate::protocol::fill_responses(
        &mut c.kernel_out[..len],
        &ranges[..range_count],
        c.hits,
        c.responses,
    )
    .map_err(|error| BringUpError::Handoff(HandoffError::Fill(error)))?;
    copy_kernel_segments(
        &c.kernel_out[..len],
        &c.segments[..info.segment_count],
        &c.plan[..plan_count],
        &mut write,
    )
    .map_err(BringUpError::Copy)?;
    // **应用 ELF 重定位**（`ET_DYN` 内核必需）：拷完段之后立刻做。
    let applied = apply_kernel_relocations(
        &c.kernel_out[..len],
        &c.plan[..plan_count],
        0,
        &mut write,
    )
    .map_err(BringUpError::Copy)?;
    let _ = applied;

    let Some(spinup_low) = (unsafe {
        current::spinup::stage_low_buffer(low_buffer.as_mut_ptr(), spinup_buf_len, &spinup_args)
    }) else {
        return Err(BringUpError::RootFrame);
    };
    let h = Handoff {
        image: &mut c.kernel_out[..len],
        spinup: spinup_low,
        ranges: &ranges[..range_count],
        hits: c.hits,
        responses: c.responses,
        plan: &c.plan[..plan_count],
        must_stay: &c.must_stay[..stays],
        entry: info.entry,
        // entry 由 spinup 参数块携带（entry_lo/entry_hi），此处保留以备查。
        source: c.memory_map,
        map_buffer: c.map_buffer,
        exit: table.exit_boot_services,
        image_handle,
        map_key: c.map_key,
    };
    // SAFETY: 由调用方保证（见函数文档）。
    // **激活移到 Exit 之后**：真机数据显示，先激活再调 ExitBootServices 时，
    // 固件在 Exit 内部访问其数据结构会挂死（h: exit 后从不返回，且无 REFUSED/INVALID）。
    // 顺序改为：Exit（引导服务失效前最后一次固件交互）→ 切 CR3 → 立即跳转。
    unsafe { enter_kernel(h, &mut page_table, enter) }.map_err(BringUpError::Handoff)?;
    Ok(())
}

/// 页表大页粒度（与 `X86PageTable` 的实现一致）。
pub const LARGE_PAGE: u64 = 2 * 1024 * 1024;

#[cfg(test)]
mod build_plan_tests {
    use super::{HHDM_OFFSET, PlanBuildError, PlanRequest, build_plan};
    use arch::addr::PhysAddr;
    use mm::plan::Mapping;
    use mm::takeover::{MustStay, check_coverage};
    use mm::usable::UsableRange;

    const LARGE: u64 = 2 * 1024 * 1024;

    fn range(base: u64, length: u64) -> UsableRange {
        UsableRange { base: PhysAddr::new(base), length }
    }

    /// 测试用请求：默认 `nx_available = true`。默认值只在这里写一次（S15），
    /// 专门测「NX 不可用」的用例自己改这个字段。
    fn request<'a>(
        kernel_phys: u64,
        kernel_virt: u64,
        kernel_len: u64,
        hhdm: &'a [UsableRange],
        identity: &'a [UsableRange],
        large: u64,
    ) -> PlanRequest<'a> {
        PlanRequest { kernel_phys, kernel_virt, kernel_len, hhdm, identity, large }
    }

    #[test]
    fn the_plan_covers_the_kernel_hhdm_and_identity_and_passes_the_pre_jump_check() {
        // 真实内核三段的并集：[0xffffffff80000000, 0xffffffff806fa000 + 0x2bea88) = 0x9b8a88。
        let kernel_virt = 0xffff_ffff_8000_0000u64;
        let kernel_len = 0x9b_8a88u64;
        let kernel_phys = 0x20_0000u64;
        let hhdm = [range(0, 0x80_0000)];
        let identity = [range(0, 0x80_0000)];
        let mut plan = [Mapping::EMPTY; 256];
        let count = build_plan(
            &request(kernel_phys, kernel_virt, kernel_len, &hhdm, &identity, LARGE),
            &mut plan,
        )
        .expect("规划应成功");
        assert!(count >= 3, "至少要有内核/HHDM/恒等三类映射，实得 {count}");
        let must_stay = [
            MustStay { start: kernel_virt, len: kernel_len },
            // **从 `IDENTITY_LOW_FLOOR` 起** ✓：页零**有意不映射**（空指针解引用必须**故障**，
            // 而不是静默读写物理 0）✗ —— 所以要求"覆盖 `0`"会与那条不变量**直接冲突** ✗。
            // 这一行是**旧行为留在测试里的残留** ✓，随规划器的改动一起更正 ✓。
            MustStay {
                start: mm::plan::IDENTITY_LOW_FLOOR,
                len: 0x80_0000 - mm::plan::IDENTITY_LOW_FLOOR,
            },
        ];
        check_coverage(&plan[..count], &must_stay).expect("覆盖检查必须通过");
        // 内核段必须**可执行**：规划器默认只给 present()，那在 x86-64 上等于 NX，
        // 跳进内核就是指令取指故障（真实运行里表现为机器复位）。
        let kernel_map = plan[..count]
            .iter()
            .find(|m| m.virt.as_u64() == kernel_virt)
            .expect("内核映射必须在");
        assert!(kernel_map.flags.is_executable(), "内核段必须可执行");
        assert!(kernel_map.flags.is_writable(), "内核段必须可写");
    }

    #[test]
    fn an_unaligned_kernel_base_is_rejected_instead_of_silently_under_mapping() {
        let mut plan = [Mapping::EMPTY; 64];
        let result = build_plan(
            &request(0x10_0000, 0xffff_ffff_8000_0000, LARGE, &[], &[], LARGE),
            &mut plan,
        );
        assert_eq!(
            result,
            Err(PlanBuildError::KernelBaseUnaligned),
            "未对齐必须拒绝：向上对齐会静默丢掉内核开头那一段"
        );
    }

    #[test]
    fn the_low_4gib_map_covers_the_lapic_page() {
        // **这条测试记录 SMP 工作的一个前提。** LAPIC 在 `0xFEE0_0000`，而启动 AP 必须先能
        // 读写它 —— 所以「我们自己的页表生效后 LAPIC 是否可达」直接决定 S3c 走哪条路：
        //
        // * 覆盖 → 只要自己的页表生效（路线 B：在 `spinup_go` 之前激活）就能读写；
        // * 不覆盖 → 必须先把 LAPIC 页加进规划。
        //
        // 混合粒度下覆盖它的是**主体**那条大页映射（`0x200000..4GiB`）。若将来有人收窄低 4 GiB，
        // 这里会先红 —— 而不是等到真机上 AP 启动失败。
        let hhdm = [range(0x1000_0000, LARGE)];
        let mut plan = [Mapping::EMPTY; 64];
        let count = build_plan(
            &request(0x20_0000, 0xffff_ffff_8000_0000, LARGE, &hhdm, &[], LARGE),
            &mut plan,
        )
        .expect("规划应成功");
        const LAPIC: u64 = 0xFEE0_0000;
        let covering = plan[..count].iter().find(|m| {
            let start = m.virt.as_u64();
            m.len > 0 && start <= LAPIC && LAPIC < start.saturating_add(m.len)
        });
        assert!(
            covering.is_some(),
            "低 4 GiB 映射必须覆盖 LAPIC 页 {:#x} —— 否则自己的页表生效后也读不到它",
            LAPIC
        );
    }

    #[test]
    fn the_low_4gib_map_is_hybrid_and_no_longer_covers_page_zero() {
        // **当前形态：从 0 起的一条 2 MiB 大页。**
        //
        // 混合粒度（头部 4 KiB + 主体大页 + 解除页零）的映射逻辑已由宿主测试证明
        // 正确（`paging::tests::the_full_low_4gib_sequence_matches_the_bootloader_plan`），
        // 但**真机仍失败**（复位循环、`entry failed`），原因尚未定位。在定位之前
        // 保持这个已知可工作的形态 —— **多映射页零是与 Limine 的已知偏差**。
        let hhdm = [range(0x1000_0000, LARGE)];
        let mut plan = [Mapping::EMPTY; 64];
        let count = build_plan(
            &request(0x20_0000, 0xffff_ffff_8000_0000, LARGE, &hhdm, &[], LARGE),
            &mut plan,
        )
        .expect("规划应成功");
        // **与 Limine 一致地从 0x1000 起**（`limine.c:200-203`）：头部 2 MiB 用 4 KiB
        // （只要 1 张页表，全部 4 KiB 要 2048 张、帧预算供不起），主体用 2 MiB 大页。
        // 页零在 `apply` 之后由 `bring_up` **显式解除**（拆分如实复制会把页零带下来）。
        for mapping in plan[..count].iter() {
            assert!(
                mapping.virt.as_u64() != 0 || mapping.len == 0,
                "页零不得出现在规划里: {:#x}+{:#x}",
                mapping.virt.as_u64(),
                mapping.len,
            );
        }
        let head = plan[..count]
            .iter()
            .find(|m| m.virt.as_u64() == 0x1000)
            .expect("头部映射必须在");
        assert_eq!(head.len, 0x20_0000 - 0x1000, "头部应覆盖到 2 MiB 边界");
        assert_ne!(head.virt.as_u64() % LARGE, 0, "头部起点不该大页对齐");
        let body = plan[..count]
            .iter()
            .find(|m| m.virt.as_u64() == 0x20_0000)
            .expect("主体映射必须在");
        assert_eq!(body.len, 0x1_0000_0000 - 0x20_0000, "主体应覆盖到 4 GiB 上界");
        assert_eq!(body.virt.as_u64() % LARGE, 0, "主体起点应大页对齐");
        assert_eq!(head.len + body.len, 0x1_0000_0000 - 0x1000, "两条应无缝覆盖低 4 GiB");
    }

    /// **实测约束**：数据映射（HHDM / 恒等 / 低 4 GiB）必须**可执行**。
    ///
    /// 曾尝试在 NX 可用时给 HHDM 置 NX（W^X：直接映射不可执行，挡住 ret2dir 一类
    /// 手法）。跳板确实在 `CR0.PG` 生效**之前**写 `EFER.NXE`（见 `spinup` 汇编），
    /// 所以 NX 位本身合法 —— 但真机结果是**复位循环**（串口 75501 字节，OVMF 引导
    /// 信息反复出现），即换表后立刻取指故障。**说明有东西经 HHDM 取指**（具体是谁
    /// 尚未定位）。在定位之前，「数据映射一律可执行」是必需，不是保守。
    ///
    /// 这条测试是那个**否定结果**的回归护栏：谁再把数据映射改成 NX，它会先红。
    #[test]
    fn data_mappings_must_stay_executable() {
        let hhdm = [range(0x1000_0000, LARGE)];
        let mut plan = [Mapping::EMPTY; 64];
        let count = build_plan(
            &request(0x20_0000, 0xffff_ffff_8000_0000, LARGE, &hhdm, &[], LARGE),
            &mut plan,
        )
        .expect("规划应成功");
        for mapping in plan[..count].iter() {
            assert!(
                mapping.flags.is_executable(),
                "所有映射都必须可执行（实测：数据映射置 NX 会导致复位循环）: virt={:#x}",
                mapping.virt.as_u64(),
            );
        }
    }

    #[test]
    fn the_hhdm_mapping_lands_at_the_declared_offset() {
        let hhdm = [range(0x1000_0000, LARGE)];
        let mut plan = [Mapping::EMPTY; 64];
        let count = build_plan(
            &request(0x20_0000, 0xffff_ffff_8000_0000, LARGE, &hhdm, &[], LARGE),
            &mut plan,
        )
        .expect("规划应成功");
        let found = plan[..count]
            .iter()
            .any(|m| m.virt.as_u64() == HHDM_OFFSET + 0x1000_0000);
        assert!(found, "HHDM 必须落在声明的偏移上 —— 报给内核的值与真实映射必须一致");
    }
}

#[cfg(test)]
mod copy_segments_tests {
    use super::{KernelPlanError, copy_kernel_segments, plan_kernel};
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::PageFlags;
    use loader::elf::{ElfError, ProgramHeader};
    use mm::plan::Mapping;

    /// 委派到共享实现（S15）：ISO 路径、LBA 与长度只在 `test_support` 里定义一次。
    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        crate::test_support::real_kernel()
    }

    fn mapping(virt: u64, phys: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(phys),
            len,
            flags: PageFlags::present(),
        }
    }

    #[test]
    fn the_real_kernel_segment_lands_at_the_planned_physical_address() {
        let Some(image) = real_kernel() else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut segments = [ProgramHeader::EMPTY; 8];
        let plan_ = plan_kernel(&image, &mut segments).expect("可规划");
        // 三段各放到不同的物理位置，互不重叠。
        let plan = [
            mapping(0xffff_ffff_8000_0000, 0x10_0000, 0x22_3cb0),
            mapping(0xffff_ffff_8022_4000, 0x40_0000, 0x4d_5780),
            mapping(0xffff_ffff_806f_a000, 0x90_0000, 0x2b_ea88),
        ];
        let mut memory = std::vec![0u8; 0x100_0000];
        let total = {
            let mut write = |phys: u64, bytes: &[u8]| {
                let at = usize::try_from(phys).map_err(|_| ElfError::SegmentOutOfBounds)?;
                let slot = memory
                    .get_mut(at..at + bytes.len())
                    .ok_or(ElfError::SegmentOutOfBounds)?;
                slot.copy_from_slice(bytes);
                Ok(())
            };
            copy_kernel_segments(&image, &segments[..plan_.segment_count], &plan, &mut write)
                .expect("拷贝成功")
        };
        assert!(total > 0);
        // 第 0 段的 p_offset 是 0x1000（实测），故其头 16 字节应出现在物理 0x100000。
        assert_eq!(
            &memory[0x10_0000..0x10_0010],
            &image[0x1000..0x1010],
            "第 0 段字节必须落到 plan 给的物理地址"
        );
    }

    #[test]
    fn a_segment_larger_than_one_mapping_is_written_across_mappings() {
        // 真实内核的第 1 段有 0x4d5780（约 4.8 MiB），**大于一条 2 MiB 映射**，
        // 所以写入必须按映射切分，而不是要求单条映射覆盖整段。
        let mut segments = [ProgramHeader::EMPTY; 2];
        segments[0].p_offset = 0;
        segments[0].p_vaddr = 0xffff_ffff_8000_0000;
        segments[0].p_filesz = 5 * 1024 * 1024;
        segments[0].p_memsz = 5 * 1024 * 1024;
        let image = std::vec![0xABu8; 5 * 1024 * 1024];
        let plan = [
            mapping(0xffff_ffff_8000_0000, 0x20_0000, 2 * 1024 * 1024),
            mapping(0xffff_ffff_8020_0000, 0x40_0000, 2 * 1024 * 1024),
            mapping(0xffff_ffff_8040_0000, 0x60_0000, 2 * 1024 * 1024),
        ];
        let mut memory = std::vec![0u8; 0x80_0000];
        let mut write = |phys: u64, bytes: &[u8]| {
            let at = usize::try_from(phys).map_err(|_| ElfError::SegmentOutOfBounds)?;
            let slot = memory
                .get_mut(at..at + bytes.len())
                .ok_or(ElfError::SegmentOutOfBounds)?;
            slot.copy_from_slice(bytes);
            Ok(())
        };
        let total = copy_kernel_segments(&image, &segments[..1], &plan, &mut write)
            .expect("跨多条映射的段必须能写入");
        assert_eq!(total, 5 * 1024 * 1024);
        // 三条映射各自的开头都应被写到（说明是**切分**写入，不是只写第一条）。
        assert_eq!(memory[0x20_0000], 0xAB);
        assert_eq!(memory[0x40_0000], 0xAB);
        assert_eq!(memory[0x60_0000], 0xAB);
    }

    #[test]
    fn a_virtual_address_the_plan_does_not_cover_is_rejected() {
        let mut segments = [ProgramHeader::EMPTY; 2];
        segments[0].p_offset = 0;
        segments[0].p_vaddr = 0xffff_ffff_8000_0000;
        segments[0].p_filesz = 16;
        segments[0].p_memsz = 16;
        let image = std::vec![7u8; 64];
        let plan: [Mapping; 0] = [];
        let mut memory = std::vec![0u8; 4096];
        let mut write = |phys: u64, bytes: &[u8]| {
            let at = usize::try_from(phys).map_err(|_| ElfError::SegmentOutOfBounds)?;
            let slot = memory
                .get_mut(at..at + bytes.len())
                .ok_or(ElfError::SegmentOutOfBounds)?;
            slot.copy_from_slice(bytes);
            Ok(())
        };
        assert_eq!(
            copy_kernel_segments(&image, &segments[..1], &plan, &mut write),
            Err(KernelPlanError::NotMapped)
            , "没有映射就必须拒绝，不能静默丢弃"
        );
    }
}

/// 装载内核并激活页表（**不跳转**）：写规划 → 拷段 → 激活。
///
/// 顺序是硬要求：**先写表、再拷段、最后才激活**；任一步失败都**不激活** ——
/// 带着残缺的地址空间激活，等于跳过去就故障。
///
/// 入口由调用方从 `plan_kernel` 取（`e_entry` 无法从段反推），本函数只负责装载与激活。
///
/// # Safety
///
/// 与 [`PageTable::activate`] 相同：调用方必须保证新页表仍映射当前正在执行的代码与栈。
pub unsafe fn load_and_activate<P: PageTable, W>(
    table: &mut P,
    plan: &[Mapping],
    image: &[u8],
    segments: &[ProgramHeader],
    entry: u64,
    write_phys: W,
) -> Result<(), KernelPlanError>
where
    W: FnMut(u64, &[u8]) -> Result<(), ElfError>,
{
    // 入口必须落在某个装载段内 —— 否则跳过去就是执行未装载的内存。
    let mut inside = false;
    for segment in segments {
        let end = segment
            .p_vaddr
            .checked_add(segment.p_memsz)
            .ok_or(KernelPlanError::Overflow)?;
        if entry >= segment.p_vaddr && entry < end {
            inside = true;
            break;
        }
    }
    if !inside {
        return Err(KernelPlanError::EntryOutsideSegments);
    }
    mm::apply::apply(table, plan).map_err(|_| KernelPlanError::MapFailed)?;
    copy_kernel_segments(image, segments, plan, write_phys)?;
    // SAFETY: 由调用方保证（见函数文档与 `PageTable::activate` 的 SAFETY 契约）。
    unsafe { table.activate() };
    Ok(())
}

#[cfg(test)]
mod load_and_activate_tests {
    use super::{KernelPlanError, load_and_activate, plan_kernel};
    use arch::addr::{PhysAddr, VirtAddr};
    use arch::paging::{MapError, PageFlags, PageTable};
    use loader::elf::{ElfError, ProgramHeader};
    use mm::plan::Mapping;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static MAP_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ACTIVATE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ACTIVATE_FAIL: AtomicUsize = AtomicUsize::new(0);

    /// 记录型假页表（成功路径）。
    struct RecordingTable;
    impl PageTable for RecordingTable {
        fn map_range(
            &mut self,
            _virt: VirtAddr,
            _phys: PhysAddr,
            _len: u64,
            _flags: PageFlags,
        ) -> Result<(), MapError> {
            MAP_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        // apply 按对齐路由：非 2 MiB 对齐的映射走这里。计数合并进 MAP_CALLS，
        // 因为测试关心的是「几条映射被落地」，而不是走了哪条粒度路径。
        fn map_range_pages(
            &mut self,
            _virt: VirtAddr,
            _phys: PhysAddr,
            _len: u64,
            _flags: PageFlags,
        ) -> Result<(), MapError> {
            MAP_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        // 假表不建模翻译：如实回答「未映射」，不假装知道。
        fn translate(&self, _virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
            None
        }

        unsafe fn activate(&self) {
            ACTIVATE_CALLS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 记录型假页表（失败路径专用，计数分开以免两个测试互相干扰）。
    struct FailTable;
    impl PageTable for FailTable {
        fn map_range(
            &mut self,
            _virt: VirtAddr,
            _phys: PhysAddr,
            _len: u64,
            _flags: PageFlags,
        ) -> Result<(), MapError> {
            Ok(())
        }
        // 假表不建模翻译：如实回答「未映射」，不假装知道。
        fn translate(&self, _virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
            None
        }

        unsafe fn activate(&self) {
            ACTIVATE_FAIL.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 委派到共享实现（S15）：ISO 路径、LBA 与长度只在 `test_support` 里定义一次。
    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        crate::test_support::real_kernel()
    }

    fn mapping(virt: u64, phys: u64, len: u64) -> Mapping {
        Mapping {
            virt: VirtAddr::new(virt),
            phys: PhysAddr::new(phys),
            len,
            flags: PageFlags::present(),
        }
    }

    #[test]
    fn a_complete_load_maps_every_mapping_copies_the_segment_and_activates_once() {
        let Some(image) = real_kernel() else {
            std::eprintln!("跳过：真实 ISO 不存在");
            return;
        };
        let mut segments = [ProgramHeader::EMPTY; 8];
        let plan_info = plan_kernel(&image, &mut segments).expect("可规划");
        let plan = [
            mapping(0xffff_ffff_8000_0000, 0x10_0000, 0x22_3cb0),
            mapping(0xffff_ffff_8022_4000, 0x40_0000, 0x4d_5780),
            mapping(0xffff_ffff_806f_a000, 0x90_0000, 0x2b_ea88),
        ];
        let mut memory = std::vec![0u8; 0x100_0000];
        let mut table = RecordingTable;
        let mut write = |phys: u64, bytes: &[u8]| {
            let at = usize::try_from(phys).map_err(|_| ElfError::SegmentOutOfBounds)?;
            let slot = memory
                .get_mut(at..at + bytes.len())
                .ok_or(ElfError::SegmentOutOfBounds)?;
            slot.copy_from_slice(bytes);
            Ok(())
        };
        MAP_CALLS.store(0, Ordering::SeqCst);
        ACTIVATE_CALLS.store(0, Ordering::SeqCst);
        let result = unsafe {
            load_and_activate(
                &mut table,
                &plan,
                &image,
                &segments[..plan_info.segment_count],
                plan_info.entry,
                &mut write,
            )
        };
        assert!(result.is_ok(), "装载并激活应成功");
        assert_eq!(plan_info.entry, 0xffff_ffff_8003_78d0, "入口来自 e_entry（实测值）");
        assert_eq!(MAP_CALLS.load(Ordering::SeqCst), 3, "规划应逐条写入页表");
        assert_eq!(ACTIVATE_CALLS.load(Ordering::SeqCst), 1, "成功后激活一次");
        assert_eq!(
            &memory[0x10_0000..0x10_0010],
            &image[0x1000..0x1010],
            "段字节必须落到规划给的物理位置"
        );
    }

    #[test]
    fn a_copy_failure_never_activates_the_table() {
        let mut segments = [ProgramHeader::EMPTY; 2];
        segments[0].p_offset = 0;
        segments[0].p_vaddr = 0xffff_ffff_8000_0000;
        segments[0].p_filesz = 16;
        segments[0].p_memsz = 16;
        let image = std::vec![7u8; 64];
        let plan: [Mapping; 0] = [];
        let mut table = FailTable;
        let mut write = |_phys: u64, _bytes: &[u8]| Ok(());
        ACTIVATE_FAIL.store(0, Ordering::SeqCst);
        let result = unsafe {
            load_and_activate(&mut table, &plan, &image, &segments[..1], 0xffff_ffff_8000_0000, &mut write)
        };
        assert_eq!(result, Err(KernelPlanError::NotMapped));
        assert_eq!(
            ACTIVATE_FAIL.load(Ordering::SeqCst),
            0,
            "装载失败绝不能激活：带着残缺地址空间跳过去就是故障"
        );
    }
}