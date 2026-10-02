//! CPU 特性探测：把「本机能不能做」变成**查出来的事实**，而不是写死的假设。
//!
//! 为什么需要它：跳板的两项行为此前是硬编码的 ——
//!
//! * `nx_available = 1` 让 32 位跳板给 `EFER` 置 `NXE`。在没有 NX 的 CPU 上，
//!   这条 `wrmsr` 是保留位写入 → `#GP`。
//! * `level5pg = 0` 假设 LA57 未启用。若固件已开启 LA57，`CR3` 会被当作 PML5
//!   解释，而我们建的是 4 级表 → 切换后取指立即失败。
//!
//! 两者在 QEMU 默认 CPU 上「恰好成立」，所以真机一直没暴露 —— 这正是硬编码假设的
//! 典型形态（S04：平台差异必须经抽象层或**探测**处理，不得依赖巧合）。

/// CPUID 叶 `0x8000_0001` 的 `EDX` 中 NX（AMD 文档里叫 `XD`）位。
pub const CPUID_EXT_FEATURES_EDX_NX: u32 = 1 << 20;
/// CPUID 叶 `0x8000_0001` 的叶号。
pub const CPUID_EXTENDED_FEATURES: u32 = 0x8000_0001;
/// 查询最大扩展叶号的 CPUID 叶号。
pub const CPUID_MAX_EXTENDED_LEAF: u32 = 0x8000_0000;
/// `CR4` 的 LA57 位（5 级分页）。
pub const CR4_LA57: u64 = 1 << 12;

/// 从 CPUID 叶 `0x8000_0001` 的 `EDX` 判定 NX 是否可用。
pub const fn nx_available_from_extended_edx(edx: u32) -> bool {
    edx & CPUID_EXT_FEATURES_EDX_NX != 0
}

/// 从 `CR4` 判定 5 级分页（LA57）是否已启用。
pub const fn la57_active_from_cr4(cr4: u64) -> bool {
    cr4 & CR4_LA57 != 0
}

/// 本机是否支持 NX。
///
/// `CPUID` 是非特权指令，宿主测试也能跑（但结果取决于测试机，所以**逻辑**用
/// `nx_available_from_extended_edx` 单独测，不依赖本函数的返回值）。
pub fn nx_available() -> bool {
    // `__cpuid` 在当前 Rust 里是安全函数：CPUID 非特权、不读写内存。
    let max_leaf = core::arch::x86_64::__cpuid(CPUID_MAX_EXTENDED_LEAF).eax;
    if max_leaf < CPUID_EXTENDED_FEATURES {
        // 不支持扩展叶 = 不存在 NX 位可查。保守返回 false。
        return false;
    }
    // 叶号已确认受支持。
    let edx = core::arch::x86_64::__cpuid(CPUID_EXTENDED_FEATURES).edx;
    nx_available_from_extended_edx(edx)
}

/// 本机是否已启用 5 级分页（LA57）。
///
/// **读 `CR4` 是特权指令**：宿主测试二进制运行在 ring 3，执行它会以
/// `STATUS_PRIVILEGED_INSTRUCTION` 崩溃整个测试进程（本项目已踩过三次）。
/// 所以这里按既有模式门控：UEFI 目标走真实读取，宿主返回 `false` 并注明
/// **这一层由真机覆盖**。
pub fn la57_active() -> bool {
    #[cfg(target_os = "uefi")]
    {
        let cr4: u64;
        // SAFETY: 引导器运行在 ring 0（UEFI 阶段），读 CR4 合法且无副作用。
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        }
        la57_active_from_cr4(cr4)
    }
    #[cfg(not(target_os = "uefi"))]
    {
        // 宿主不执行特权指令；真实值由真机运行覆盖（PRE-2）。
        false
    }
}

/// 读时间戳计数器（TSC）✓。
///
/// **为什么在实现层**：`rdtsc` 是 x86 指令 ✓ —— 让它出现在中性层就是跨层直连 ✗（S14 / ADR-007）✓。
///
/// **为什么不用固件的 `GetNextMonotonicCount`**：真机**两次实测**（第 153/154 轮）
/// **停 10 毫秒只涨 1** ✗ —— OVMF/QEMU 下它不是可用的时间源 ✓。TSC 是**已知会走**的计数器 ✓。
///
/// **频率未知** ✗ —— 由调用方用**已知时长**（固件的 `Stall`）实测标定 ✓，见 `hz_from_ticks`。
#[cfg(target_os = "uefi")]
#[inline]
pub unsafe fn read_tsc() -> u64 {
    // SAFETY: `rdtsc` 在任何 x86-64 上都可执行，且**无副作用** ✓（只是读一个计数器）。
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// 由"**已知时长**内走过的 TSC 数"算出频率（Hz）—— **纯逻辑、宿主可测** ✓。
///
/// `micros == 0` 返回 `None` ✗ —— **不返回编造的数** ✓，也不 panic ✓。
#[inline]
pub const fn hz_from_ticks(ticks: u64, micros: u64) -> Option<u64> {
    if micros == 0 {
        return None;
    }
    Some(ticks.saturating_mul(1_000_000) / micros)
}

/// 由 TSC 数与频率算出**微秒** —— **纯逻辑、宿主可测** ✓。
///
/// `hz == 0` 返回 `None` ✗（**不除零、不 panic** ✓）。
#[inline]
pub const fn micros_from_ticks(ticks: u64, hz: u64) -> Option<u64> {
    if hz == 0 {
        return None;
    }
    Some(ticks.saturating_mul(1_000_000) / hz)
}

#[cfg(test)]
mod tests {
    use super::{
        CPUID_EXT_FEATURES_EDX_NX, CR4_LA57, hz_from_ticks, la57_active_from_cr4,
        micros_from_ticks, nx_available_from_extended_edx,
    };

    #[test]
    fn the_tsc_calibration_refuses_to_invent_a_number() {
        // **零时长得不到频率** ✗ —— 返回 `None` 而不是一个编造的数 ✓（S09）。
        assert_eq!(hz_from_ticks(1_000_000, 0), None);
        assert_eq!(micros_from_ticks(1_000_000, 0), None, "不得除零");
        assert_eq!(hz_from_ticks(1_000, 1), Some(1_000_000_000), "1 微秒 1000 个数 → 1 GHz");
        assert_eq!(hz_from_ticks(30_000_000, 10_000), Some(3_000_000_000), "10 毫秒 → 3 GHz");
        assert_eq!(micros_from_ticks(3_000_000, 3_000_000_000), Some(1_000), "3 GHz 下 1 毫秒");
        // **饱和而不是回绕** ✗ —— 回绕会给出一个极小值，看起来像"极快" ✗。
        assert_eq!(hz_from_ticks(u64::MAX, 1), Some(u64::MAX));
    }

    #[test]
    fn nx_is_read_from_the_documented_bit() {
        assert!(!nx_available_from_extended_edx(0), "全零 EDX 表示不支持 NX");
        assert!(nx_available_from_extended_edx(CPUID_EXT_FEATURES_EDX_NX));
        // 其它位不得误判为 NX。
        assert!(!nx_available_from_extended_edx(!CPUID_EXT_FEATURES_EDX_NX));
    }

    #[test]
    fn la57_is_read_from_the_documented_bit() {
        assert!(!la57_active_from_cr4(0));
        assert!(la57_active_from_cr4(CR4_LA57));
        // 其它 CR4 位（如 PAE=bit5）不得误判为 LA57。
        assert!(!la57_active_from_cr4(1 << 5));
        assert!(la57_active_from_cr4((1 << 5) | CR4_LA57));
    }

    #[test]
    fn probing_nx_does_not_panic_on_this_machine() {
        // CPUID 非特权，宿主可执行；只断言「能问出一个布尔值」而不假设其取值 ——
        // 测试机可能支持也可能不支持 NX，假设任一取值都是把测试绑到硬件上。
        let _ = super::nx_available();
    }
}