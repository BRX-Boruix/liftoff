//! 入口编排（可宿主测试的部分）。
//!
//! 边界：bin 只做 `efi_main` 转发；本模块做“取引导服务表 → 输出启动诊断”的编排。
//! 平台与固件实现的**选择**来自门面（`crate::PlatformImpl`、`firmware_current::current`），
//! 本模块不自己挑实现。

use arch::paging::PageTable;
use loader::elf::{ElfError, ProgramHeader, load_segments, parse_elf_header, parse_load_segments};
use arch::platform::Platform;
use firmware::boot_services::BootServicesControl;
use firmware::memory::{MemoryEntry, MemoryMapSource};
use crate::responses::Responses;
use limine::scan::RequestHit;
use mm::plan::Mapping;
use mm::takeover::{MustStay, TakeoverError};
use firmware::error::Error;
use firmware_current::current::{
    ExitBootServices, Handle, SystemTable, UefiBootServices, UefiMemoryMapSource, boot_services_of,
};

/// 入口第一步的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// 引导服务表已取得，启动诊断已输出。
    Ready,
}

/// 以指定平台执行入口第一步；失败时**不输出诊断**（不制造假成功）。
pub fn start_with<P: Platform>(system_table: *mut SystemTable) -> Result<Outcome, Error> {
    let _boot_services = boot_services_of(system_table)?;
    crate::diag::report_startup::<P>();
    Ok(Outcome::Ready)
}

/// 生产入口：平台固定为门面选定的实现。
pub fn start(system_table: *mut SystemTable) -> Result<Outcome, Error> {
    start_with::<crate::PlatformImpl>(system_table)
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
    let map = source.memory_map(map_buffer)?;
    let count = map.len();
    if !capture_map_key(source, map_key) {
        return Err(Error::InvalidState);
    }
    // SAFETY: 由调用方保证（见函数文档与 `exit_prepared` 的 SAFETY 契约）。
    unsafe { exit_prepared(exit, image_handle, map_key)? };
    Ok(count)
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

    fn table_with_boot_services() -> SystemTable {
        // SAFETY: 零值是合法表示（全空指针）。
        let mut table = unsafe { core::mem::zeroed::<SystemTable>() };
        table.boot_services = 0x40usize as *mut core::ffi::c_void;
        table
    }

    #[test]
    fn a_null_system_table_fails_without_writing_anything() {
        reset();
        assert_eq!(start_with::<Recorder>(core::ptr::null_mut()), Err(Error::InvalidArgument));
        assert!(taken().is_empty(), "失败时不得输出诊断（不制造假成功）");
    }

    #[test]
    fn a_valid_system_table_writes_the_startup_line() {
        reset();
        let mut table = table_with_boot_services();
        assert_eq!(start_with::<Recorder>(&mut table), Ok(Outcome::Ready));
        let line = std::string::String::from_utf8(taken()).expect("UTF-8");
        assert_eq!(line, "[liftoff] gen2 up, platform=recorder\n");
    }

    #[test]
    fn the_production_entry_uses_the_selected_platform() {
        // 生产入口只把平台固定为门面选定的实现；其行为由 QEMU 验收（PRE-2）。
        let _ = start as fn(*mut SystemTable) -> Result<Outcome, Error>;
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
pub unsafe fn enter_kernel<F>(h: Handoff<'_, '_>, enter: F) -> Result<usize, HandoffError>
where
    F: FnOnce(u64) -> !,
{
    let report = crate::protocol::fill_responses(h.image, h.hits, h.responses)
        .map_err(HandoffError::Fill)?;
    let entry =
        check_before_entry(h.entry, h.plan, h.must_stay).map_err(HandoffError::BeforeEntry)?;
    // 映射条目数只作诊断：本函数**必然发散**（`enter` 的返回类型是 `!`），故显式标记为有意不用。
    let _count = unsafe { handoff(h.source, h.map_buffer, h.exit, h.image_handle, h.map_key) }
        .map_err(HandoffError::Exit)?;
    let _ = report;
    enter(entry);
}

#[cfg(test)]
mod enter_kernel_tests {
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
        let result = unsafe { enter_kernel(h, counting_enter) };
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
        let result = unsafe { enter_kernel(h, counting_enter) };
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
            enter_kernel(h, recording_enter)
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
            let len = bytes.len() as u64;
            let end = vaddr.checked_add(len).ok_or(ElfError::SegmentOutOfBounds)?;
            // 必须落在**同一条**映射内（不允许跨映射拼接）。
            let mut found = None;
            for candidate in plan {
                let Some(mapping_end) = candidate.virt.as_u64().checked_add(candidate.len) else {
                    continue;
                };
                if vaddr >= candidate.virt.as_u64() && end <= mapping_end {
                    found = Some(candidate);
                    break;
                }
            }
            let Some(mapping) = found else {
                not_mapped = true;
                return Err(ElfError::SegmentOutOfBounds);
            };
            let offset = vaddr - mapping.virt.as_u64();
            let phys = mapping
                .phys
                .as_u64()
                .checked_add(offset)
                .ok_or(ElfError::SegmentOutOfBounds)?;
            write_phys(phys, bytes)?;
            total += bytes.len();
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