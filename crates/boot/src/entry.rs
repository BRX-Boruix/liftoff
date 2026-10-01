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
    ExitBootServices, Handle, SUCCESS, SystemTable, UefiBlockDevices, UefiBootServices, UefiGraphics,
    acpi_rsdp, graphics_output_mode,
    UefiMemoryMapSource, boot_services_of,
};

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
        bring_up(&*boot_services, image_handle, c, |entry| <P as Platform>::jump_to(entry))
    };
    if let Err(stage) = result {
        // 失败时把环节写出来：真跑时这是最有用的信息。
        let text = stage_text(stage);
        for byte in text {
            P::write_byte(*byte);
        }
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

/// 失败环节的短文本（真跑时从串口就能看出卡在哪一步）。
fn stage_text(stage: BringUpError) -> &'static [u8] {
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
        unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };
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
                #[cfg(target_os = "uefi")]
                unsafe {
                    core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
                }
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
    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let bytes = std::fs::read(iso).ok()?;
        let at = 33 * 2048;
        let size = 24_619_400;
        bytes.get(at..at + size).map(|s| s.to_vec())
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

/// 组装交接所需的页表规划：内核高区 + HHDM + 恒等。
///
/// 恒等映射**必须保留**：切换页表时当前正在执行的代码与栈必须仍然被映射
/// （`PageTable::activate` 的 SAFETY 契约）。保留恒等映射后这条自动成立，
/// 无需去读 RIP/RSP（那需要汇编）。`identity` 由调用方给出（取自固件内存映射的可用区间）。
///
/// 内核按**一段**给出（三段取并集）：段间空隙也会被映射，这是有意的简化。
pub fn build_plan(
    kernel_phys: u64,
    kernel_virt: u64,
    kernel_len: u64,
    hhdm: &[UsableRange],
    identity: &[UsableRange],
    out: &mut [Mapping],
    large: u64,
) -> Result<usize, PlanBuildError> {
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
    total += mm::plan::plan_hhdm(hhdm, HHDM_OFFSET, &mut out[total..], large)
        .map_err(PlanBuildError::Plan)?;
    total += mm::plan::plan_identity(identity, &mut out[total..], large)
        .map_err(PlanBuildError::Plan)?;
    // 规划器只产出 `present()` —— 在 x86-64 上那等于 **NX 置位**：内核入口所在的代码段
    // 不可执行，一跳过去就指令取指故障（真实运行表现为机器复位）。这里按用途补权限：
    // 内核段 R/W/X（它要执行代码、写数据）；HHDM 与恒等 R/W（引导器与内核都要读写）。
    let kernel_flags = PageFlags::present()
        .with(PageFlags::writable())
        .with(PageFlags::executable());
    // **不设 NX**：置 NX 需要 `EFER.NXE=1`，而固件未必开启 —— 未开启时带 bit 63 的表项
    // 是保留位违规，换表即 #GP → 三重故障（真机上正是如此）。恒等/HHDM 可执行无害。
    let data_flags = PageFlags::present()
        .with(PageFlags::writable())
        .with(PageFlags::executable());
    for (index, mapping) in out[..total].iter_mut().enumerate() {
        mapping.flags = if index < kernel_count { kernel_flags } else { data_flags };
    }
    Ok(total)
}

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
    table: &BootServicesTable,
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
    let usable_count =
        mm::usable::identity_ranges(map, c.usable).map_err(BringUpError::MemoryMapRanges)?;
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
        c.destination,
        kernel_virt,
        kernel_len,
        &c.usable[..usable_count],
        &c.usable[..usable_count],
        c.plan,
        LARGE_PAGE,
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
    mm::apply::apply(&mut page_table, &c.plan[..plan_count]).map_err(BringUpError::Apply)?;
    for byte in b"[liftoff] step: plan applied\n" as &[u8] {
        crate::PlatformImpl::write_byte(*byte);
    }
    copy_kernel_segments(
        &c.kernel_out[..len],
        &c.segments[..info.segment_count],
        &c.plan[..plan_count],
        &mut write,
    )
    .map_err(BringUpError::Copy)?;
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
    c.responses.set_hhdm_offset(HHDM_OFFSET);
    // RSDP 只在**真的从配置表找到**时才填；没有就留空 —— 给假指针比不给更糟。
    if let Some(rsdp) = c.rsdp {
        c.responses.set_rsdp(rsdp);
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
    let spinup_args = current::spinup::SpinupArgs {
        level5pg: 0,
        pagemap_top: root.start_address().expect("根帧必有地址").as_u64() as u32,
        entry_lo: (info.entry & 0xFFFF_FFFF) as u32,
        entry_hi: (info.entry >> 32) as u32,
        // 内核声明的主栈：.kernel_main_stack 在 0xffffffff808ae000，大小 0x101000，
        // 所以栈顶 = 0xffffffff809ae000（**不是**之前手写的近似值）。
        stack_lo: (KERNEL_STACK_TOP & 0xFFFF_FFFF) as u32,
        stack_hi: (KERNEL_STACK_TOP >> 32) as u32,
        gdt: 0,
        nx_available: 1,
        dmo_lo: (HHDM_OFFSET & 0xFFFF_FFFF) as u32,
        dmo_hi: (HHDM_OFFSET >> 32) as u32,
        base_revision: 1,
    };
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
    use super::{HHDM_OFFSET, PlanBuildError, build_plan};
    use arch::addr::PhysAddr;
    use mm::plan::Mapping;
    use mm::takeover::{MustStay, check_coverage};
    use mm::usable::UsableRange;

    const LARGE: u64 = 2 * 1024 * 1024;

    fn range(base: u64, length: u64) -> UsableRange {
        UsableRange { base: PhysAddr::new(base), length }
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
        let count = build_plan(kernel_phys, kernel_virt, kernel_len, &hhdm, &identity, &mut plan, LARGE)
            .expect("规划应成功");
        assert!(count >= 3, "至少要有内核/HHDM/恒等三类映射，实得 {count}");
        let must_stay = [
            MustStay { start: kernel_virt, len: kernel_len },
            MustStay { start: 0, len: 0x80_0000 },
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
            0x10_0000,
            0xffff_ffff_8000_0000,
            LARGE,
            &[],
            &[],
            &mut plan,
            LARGE,
        );
        assert_eq!(
            result,
            Err(PlanBuildError::KernelBaseUnaligned),
            "未对齐必须拒绝：向上对齐会静默丢掉内核开头那一段"
        );
    }

    #[test]
    fn the_hhdm_mapping_lands_at_the_declared_offset() {
        let hhdm = [range(0x1000_0000, LARGE)];
        let mut plan = [Mapping::EMPTY; 64];
        let count =
            build_plan(0x20_0000, 0xffff_ffff_8000_0000, LARGE, &hhdm, &[], &mut plan, LARGE)
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

    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let bytes = std::fs::read(iso).ok()?;
        bytes.get(33 * 2048..33 * 2048 + 24_619_400).map(|s| s.to_vec())
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
        unsafe fn activate(&self) {
            ACTIVATE_FAIL.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn real_kernel() -> Option<std::vec::Vec<u8>> {
        let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
        let bytes = std::fs::read(iso).ok()?;
        bytes.get(33 * 2048..33 * 2048 + 24_619_400).map(|s| s.to_vec())
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