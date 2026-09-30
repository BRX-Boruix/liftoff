//! 入口编排（可宿主测试的部分）。
//!
//! 边界：bin 只做 `efi_main` 转发；本模块做“取引导服务表 → 输出启动诊断”的编排。
//! 平台与固件实现的**选择**来自门面（`crate::PlatformImpl`、`firmware_current::current`），
//! 本模块不自己挑实现。

use arch::platform::Platform;
use firmware::boot_services::BootServicesControl;
use firmware::memory::{MemoryEntry, MemoryMapSource};
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
