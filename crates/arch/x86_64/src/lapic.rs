//! x86 LAPIC：APIC 标识与 **IPI 命令寄存器（ICR）编码**（S3 的纯逻辑部分）。
//!
//! **为什么先做这一半**：S3 的其余部分（读写 MMIO / MSR）**只能真机验证**，而 ICR 的位编码
//! 是**纯逻辑**。而位编码恰恰是 AP 启动最容易错的地方 —— 错了的表现是"IPI 发给了错误的核"或
//! "AP 永远不醒"，在真机上都是**静默复位**，极难定位。所以先把这一半测透。
//!
//! **两种模式的 ICR 布局不同**（这是本模块存在的核心理由）：
//! * xAPIC：ICR 是 MMIO 的两个 32 位寄存器；**目的地在高 32 位寄存器的高 8 位**，
//!   合并成 64 位视图就是 **bit 56**。
//! * x2APIC：ICR 是一个 64 位 MSR；**目的地在 bit 32**。
//! 混用这两种布局不会报错，只会**把 IPI 发给别的核**。

/// APIC 标识。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApicId(pub u32);

impl ApicId {
    /// 全播（**不含自己**）—— ACPI/x86 规范常量，不是随手写的数。
    pub const ALL_BUT_SELF: ApicId = ApicId(0xFFFF_FFFF);
}

/// APIC 工作模式。**ICR 的位布局随它不同**，所以必须显式建模而不是靠调用方记住。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApicMode {
    /// MMIO 访问的传统模式（目的地 8 位）。
    Xapic,
    /// MSR 访问的扩展模式（目的地 32 位）。
    X2apic,
}

/// 投递模式（ICR bits 8..10）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeliveryMode {
    Fixed = 0,
    LowestPriority = 1,
    Smi = 2,
    Nmi = 4,
    Init = 5,
    Startup = 6,
}

/// 向量在 ICR 低 8 位。
pub const ICR_VECTOR_MASK: u64 = 0xFF;
/// 投递模式从 bit 8 起。
pub const ICR_DELIVERY_SHIFT: u32 = 8;
/// 投递模式占 3 位。
pub const ICR_DELIVERY_MASK: u64 = 0x7;
/// assert（电平有效）。
pub const ICR_LEVEL_ASSERT: u64 = 1 << 14;
/// 电平触发。
pub const ICR_TRIGGER_LEVEL: u64 = 1 << 15;
/// xAPIC：目的地在该 32 位寄存器的高 8 位 → 合并 64 位视图的 bit 56。
pub const ICR_XAPIC_DEST_SHIFT: u32 = 56;
/// x2APIC：目的地在 bit 32。
pub const ICR_X2APIC_DEST_SHIFT: u32 = 32;

/// ICR 编码失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApicError {
    /// xAPIC 的目的地只有 8 位；超出即**拒绝**。
    ///
    /// **不得截断**：截断会把 IPI 发给**另一个核**，而调用方以为发对了。
    DestinationTooLargeForXapic,
    /// 该 CPU **永久**关闭了 xAPIC，回退到 xAPIC 不可能成功。
    ///
    /// **不是"操作失败"**：是硬件状态决定了这条路不存在，只能如实上报。
    XapicPermanentlyDisabled,
    /// 两步写完了，但复读 MSR 发现 x2APIC **仍然开着**。
    ///
    /// **必须复核**：不复核就可能在 x2APIC 仍生效时按 xAPIC 发 IPI，
    /// 而那不会报错，只会**什么都不发生**。
    RevertDidNotTakeEffect,
}

impl core::fmt::Display for ApicError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DestinationTooLargeForXapic => {
                f.write_str("xAPIC 的目的地只有 8 位，超出即拒绝（截断会发给错误的核）")
            }
            Self::XapicPermanentlyDisabled => {
                f.write_str("该 CPU 永久关闭了 xAPIC，无法从 x2APIC 回退")
            }
            Self::RevertDidNotTakeEffect => {
                f.write_str("写完了 MSR 但 x2APIC 仍开着，回退未生效")
            }
        }
    }
}

/// xAPIC 的 LAPIC ID 寄存器偏移。
pub const LAPIC_ID: u32 = 0x020;
/// xAPIC 的 ICR **低半**（写它才触发发送）。
pub const LAPIC_ICR_LOW: u32 = 0x300;
/// xAPIC 的 ICR **高半**（目的地在这里）。
pub const LAPIC_ICR_HIGH: u32 = 0x310;
/// xAPIC 的默认 MMIO 基址（MADT 未给时用）。
pub const LAPIC_DEFAULT_BASE: u64 = 0xFEE0_0000;
/// x2APIC 的 ID 寄存器 MSR。
pub const X2APIC_MSR_ID: u32 = 0x802;
/// x2APIC 的 ICR MSR。
pub const X2APIC_MSR_ICR: u32 = 0x830;

/// LAPIC 的访问方式。**两种方式的寄存器位置与位宽都不同**，所以显式建模。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApicAccess {
    /// xAPIC：MMIO 的两个 32 位寄存器。
    Xapic { base: u64 },
    /// x2APIC：一个 64 位 MSR。
    X2apic,
}

/// 一次寄存器写入。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegisterWrite {
    /// 写 32 位 MMIO 寄存器。
    Mmio32 { address: u64, value: u32 },
    /// 写 MSR。
    Msr { index: u32, value: u64 },
}

/// ICR 的**有序**写入序列。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IcrSequence {
    /// 必须先做的写入。
    pub first: RegisterWrite,
    /// 之后要做的写入（x2APIC 只有一次）。
    pub second: Option<RegisterWrite>,
}

/// 把 ICR 值转成**有序的写入序列**。
///
/// **顺序是语义的一部分，不是风格问题。** xAPIC 下 ICR 是两个 MMIO 寄存器，而
/// **写低半（ICR0）才触发发送** —— 所以必须先写高半（ICR1，含目的地），再写低半。
/// 反过来写的话，IPI 会带着**上一次留在 ICR1 里的目的地**发出去，即**发给错误的核**；
/// 真机上表现为静默复位。brxLimine 正是 `ICR1` 然后 `ICR0`（`common/sys/smp.c:88-89`）。
pub fn icr_writes(access: ApicAccess, value: u64) -> IcrSequence {
    match access {
        ApicAccess::Xapic { base } => IcrSequence {
            // **先写高半**（目的地在该寄存器的 bits 24-31），因为**写低半才触发发送**。
            first: RegisterWrite::Mmio32 {
                address: base + LAPIC_ICR_HIGH as u64,
                value: (value >> 32) as u32,
            },
            second: Some(RegisterWrite::Mmio32 {
                address: base + LAPIC_ICR_LOW as u64,
                value: value as u32,
            }),
        },
        ApicAccess::X2apic => IcrSequence {
            first: RegisterWrite::Msr { index: X2APIC_MSR_ICR, value },
            second: None,
        },
    }
}

/// ICR 的**投递状态**位（bit 12）：1 = 上一次 IPI 尚未送达。
pub const ICR_DELIVERY_STATUS: u64 = 1 << 12;

/// ICR 是否**仍在投递中**（纯逻辑、宿主可测）。
pub const fn icr_busy(icr: u64) -> bool {
    icr & ICR_DELIVERY_STATUS != 0
}

/// 等待投递完成的自旋上限（对照 brxLimine `lapic.c:300`：一百万次 + `pause`）。
///
/// **必须有界**：无界自旋在硬件异常时会把引导器挂死，而串口上什么都不显示。
pub const ICR_WAIT_SPINS: u32 = 1_000_000;

/// INIT assert 的 ICR 值（对照 brxLimine `smp.c:85` 的 `0x4500`）。
///
/// **由 `icr_value` 派生并由测试绑定**（S13 单点）—— 不手写魔数。
pub const IPI_INIT_ASSERT: u64 = 0x4500;
/// INIT **deassert** 的 ICR 值。
///
/// 【引用修正】我上一版把它写成"对照 `smp.c:96` 的 `0x0500`"——**那是错的**：
/// `smp.c:96` 是 SIPI 那一条，而整个 `common/sys/smp.c` 里**根本没有** `0x0500`
/// （已逐行核实）。参考实现只写 `0x4500` 就 `stall(10000)`（`smp.c:89,91`）——
/// 它**不发** deassert。
///
/// 我们仍然发它：Intel SDM Vol 3 §10.6.1 的 INIT 电平语义要求 assert 之后 deassert；
/// 而多写一次是**无害**的（电平位不生效时它与 assert 同形，等于多一次 INIT）。
/// 这是**有意与参考实现不同**的一处，记录在此。
pub const IPI_INIT_DEASSERT: u64 = 0x0500;
/// SIPI 的 ICR 基础值（投递模式 Startup + assert）；向量按位或进去。
pub const IPI_SIPI_BASE: u64 = 0x4600;

/// INIT 之后等待的固件延时（微秒）。参考实现 `stall(10000)` = 10 ms。
pub const AP_INIT_STALL_US: usize = 10_000;
/// 两次 SIPI 之间等待的固件延时（微秒）。参考实现 `stall(200)`。
pub const AP_SIPI_STALL_US: usize = 200;
/// 等 AP 写 `booted_flag` 的**轮询次数**。参考实现 100 次（`smp.c:112`）。
pub const AP_BOOT_POLLS: u32 = 100;
/// 每次轮询之间的固件延时（微秒）。参考实现 `stall(10000)` = 10 ms。
/// 于是总超时 = 100 × 10 ms = **1 秒**—— **有界**，绝不无限自旋。
pub const AP_BOOT_STALL_US: usize = 10_000;

/// `IA32_APIC_BASE` MSR（`0x1B`）—— xAPIC/x2APIC 的模式开关就在这里。
pub const IA32_APIC_BASE: u32 = 0x1B;

/// `IA32_APIC_BASE` bit 11：APIC **全局**使能。
pub const APIC_BASE_ENABLE: u64 = 1 << 11;

/// `IA32_APIC_BASE` bit 10：**x2APIC** 使能。
pub const APIC_BASE_X2APIC: u64 = 1 << 10;

/// `IA32_ARCH_CAPABILITIES`（架构能力 MSR）。
pub const IA32_ARCH_CAPABILITIES: u32 = 0x10A;
/// 该 MSR 的 bit 21：支持 `XAPIC_DISABLE` 特性（Intel Meteor Lake 及以后）。
pub const ARCH_CAPS_XAPIC_DISABLE: u64 = 1 << 21;
/// `IA32_XAPIC_DISABLE_STATUS`：bit 0 表示 xAPIC 已被**永久**关闭。
pub const IA32_XAPIC_DISABLE_STATUS: u32 = 0xBD;
/// 该 MSR 的 bit 0。
pub const XAPIC_DISABLE_STATUS_PERMANENT: u64 = 1;
/// CPUID 叶 7 子叶 0 的 `EDX` bit 29：存在 `IA32_ARCH_CAPABILITIES`。
pub const CPUID_7_0_EDX_ARCH_CAPABILITIES: u32 = 1 << 29;

/// 读一个 MSR。
///
/// **只在 UEFI 目标上存在**：`rdmsr` 是特权指令，宿主测试里执行会让整个测试进程崩掉。
///
/// # Safety
///
/// `index` 必须是**当前 CPU 上存在**的 MSR —— 读不存在的 MSR 会 `#GP`。
/// `IA32_APIC_BASE` 在 x86-64 上必然存在，读它是安全的。
#[cfg(target_os = "uefi")]
pub unsafe fn rdmsr(index: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: 由调用方保证 MSR 存在（见函数文档）。
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") index,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | low as u64
}

/// 从 `IA32_APIC_BASE` 判断 x2APIC 是否**真的**已启用。
///
/// **两个位都要看，只看 bit 10 会误判**：bit 10 只有在 bit 11（APIC 全局使能）也为 1 时才生效。
/// 固件把 APIC 整体关着、而 bit 10 残留为 1 是完全可能的 —— 那时按 x2APIC 去访问会得到
/// 一个"看起来能读、实际无效"的结果，比直接失败更难查。
pub const fn x2apic_enabled(apic_base: u64) -> bool {
    apic_base & APIC_BASE_ENABLE != 0 && apic_base & APIC_BASE_X2APIC != 0
}

/// 打开 x2APIC 后的 `IA32_APIC_BASE` 值。
///
/// **同时置 bit 11**：只置 bit 10 而不开 APIC 全局使能，等于把开关放在一个关着的总闸后面。
pub const fn with_x2apic_enabled(apic_base: u64) -> u64 {
    apic_base | APIC_BASE_ENABLE | APIC_BASE_X2APIC
}

/// 关掉 x2APIC（退回 xAPIC）后的值。
///
/// **只清 bit 10，保留 bit 11**：LAPIC 仍要通过 MMIO 访问，全局使能不能关。
pub const fn with_x2apic_disabled(apic_base: u64) -> u64 {
    (apic_base | APIC_BASE_ENABLE) & !APIC_BASE_X2APIC
}

/// 从 x2APIC 退回 xAPIC 的**两步** MSR 值；本来就没生效时返回 `None`。
///
/// **为什么必须两步**：x2APIC 直接切 xAPIC 是**非法状态转换**（`#GP`）。
/// 参考实现（`common/sys/lapic.c:376-381`）先**同时**清 bit 10 与 bit 11（APIC 全关），
/// 再单独置 bit 11 打开 xAPIC。所以 `with_x2apic_disabled` 算出的**最终值是对的**，
/// 但**不能一次写进去** —— 这正是 S4 缺的另一半。
///
/// **判定比参考实现更严**：参考只看 bit 10（`rdmsr(0x1b) & (1 << 10)`），
/// 我们用 [`x2apic_enabled`]（两个位都要）—— bit 10 残留但全局使能关着时 x2APIC
/// 并未生效，那时**没有可退的东西**，返回 `None` 才是如实的。
pub const fn xapic_revert_steps(apic_base: u64) -> Option<(u64, u64)> {
    if !x2apic_enabled(apic_base) {
        return None;
    }
    let disabled = apic_base & !(APIC_BASE_ENABLE | APIC_BASE_X2APIC);
    Some((disabled, disabled | APIC_BASE_ENABLE))
}

/// xAPIC 是否被**永久**关闭（Meteor Lake 及以后）。
///
/// 为真时回退**不可能**成功，必须**先查再写** —— 往这种 CPU 的该 MSR 写非法值会 `#GP`。
pub const fn xapic_permanently_disabled(
    arch_capabilities_present: bool,
    arch_capabilities: u64,
    disable_status: u64,
) -> bool {
    arch_capabilities_present
        && arch_capabilities & ARCH_CAPS_XAPIC_DISABLE != 0
        && disable_status & XAPIC_DISABLE_STATUS_PERMANENT != 0
}

/// 选访问方式：固件是否已启用 x2APIC。
///
/// **不是"有 x2APIC 就用"**：brxLimine 的语义是「**内核**是否支持 x2APIC」—— 内核不支持时
/// 它会把固件的 x2APIC 退回 xAPIC（`smp.c:144-153`）。内核是否支持由协议请求的标志告知，
/// 所以这里把判断显式化成参数，而不是由本模块替内核决定。
pub fn select_access(kernel_supports_x2apic: bool, firmware_x2apic_enabled: bool) -> ApicAccess {
    if kernel_supports_x2apic && firmware_x2apic_enabled {
        ApicAccess::X2apic
    } else {
        ApicAccess::Xapic { base: LAPIC_DEFAULT_BASE }
    }
}

/// 构造 IPI 命令寄存器（ICR）的 64 位值。
///
/// 返回值可直接写入：x2APIC 写整个 MSR；xAPIC 写低 32 位到 `+0x300`、高 32 位到 `+0x310`
/// （即 `value as u32` 与 `(value >> 32) as u32`）。
/// **对照 brxLimine 核实过**（`common/sys/smp.c:85,101`）：
/// * INIT assert 写作 `0x4500` = 投递模式 5 << 8 | bit 14 —— **不含 bit 15（电平）**；
/// * SIPI 写作 `vector | 0x4600` = 向量 | 投递模式 6 << 8 | bit 14。
///
/// 我原先以为 INIT 要置电平位（bit 15）—— **查参考实现后确认不是**，故按它来。
pub fn icr_value(
    mode: ApicMode,
    dest: ApicId,
    delivery: DeliveryMode,
    vector: u8,
    assert: bool,
) -> Result<u64, ApicError> {
    let dest_shift = match mode {
        ApicMode::Xapic => {
            // xAPIC 的目的地只有 8 位。**拒绝而不是截断**：截断会把 IPI 发给另一个核，
            // 而调用方以为发对了 —— 真机上表现为静默复位，极难定位。
            if dest.0 > 0xFF {
                return Err(ApicError::DestinationTooLargeForXapic);
            }
            ICR_XAPIC_DEST_SHIFT
        }
        ApicMode::X2apic => ICR_X2APIC_DEST_SHIFT,
    };
    let mut value = (dest.0 as u64) << dest_shift;
    value |= (vector as u64) & ICR_VECTOR_MASK;
    value |= ((delivery as u64) & ICR_DELIVERY_MASK) << ICR_DELIVERY_SHIFT;
    if assert {
        value |= ICR_LEVEL_ASSERT;
    }
    Ok(value)
}

/// 读一个 32 位 MMIO 寄存器。
///
/// # Safety
/// `address` 必须是**已映射**的 MMIO（LAPIC 在低 4 GiB 恒等映射内），且为 4 字节对齐。
#[cfg(target_os = "uefi")]
pub unsafe fn mmio_read32(address: u64) -> u32 {
    // SAFETY: 由调用方保证（见函数文档）。
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

/// 固件当前配置的本地 APIC 状态（诊断与选路用）。
///
/// **为什么要有它**：中性层此前**自己读 MSR `0x1B` 并解析位域**—— 那既把 MSR 编号
/// 泄漏进中性层（ADR-007），也让"哪些位是什么意思"有了**第二个出处**（S13 单点）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApicState {
    /// `IA32_APIC_BASE` 的原始值（诊断打印用）。
    pub base: u64,
    /// 是否**真的**启用了 x2APIC（**两位都要看**）。
    pub x2apic: bool,
    /// APIC 全局使能位（bit 11）。
    pub global_enable: bool,
}

impl ApicState {
    /// 由 `IA32_APIC_BASE` 的原始值解析 —— **纯逻辑、宿主可测**。
    #[inline]
    pub const fn from_base(base: u64) -> Self {
        Self {
            base,
            x2apic: x2apic_enabled(base),
            global_enable: base & APIC_BASE_ENABLE != 0,
        }
    }
}

/// 读固件当前的本地 APIC 状态。
///
/// # Safety
/// 读 MSR `IA32_APIC_BASE` —— 该 MSR 在 x86-64 上**必然存在**，故无故障风险。
#[cfg(target_os = "uefi")]
pub unsafe fn firmware_apic_state() -> ApicState {
    // SAFETY: 见函数文档。
    ApicState::from_base(unsafe { rdmsr(IA32_APIC_BASE) })
}

/// 固件当前要求的 APIC 访问方式（结合"内核是否支持 x2APIC"）。
///
/// **中性层据此发 IPI**—— 它不必知道 MSR 编号，也不必解析位域。
///
/// # Safety
/// 同 [`firmware_apic_state`]。
#[cfg(target_os = "uefi")]
pub unsafe fn firmware_access(kernel_supports_x2apic: bool) -> ApicAccess {
    let state = unsafe { firmware_apic_state() };
    select_access(kernel_supports_x2apic, state.x2apic)
}

/// 从 `LAPIC_ID` 寄存器的**原始 32 位值**里取出 APIC 标识（高 8 位）。
///
/// **纯逻辑、宿主可测**—— 移位写错不会崩，只会得到一个"看起来像标识"的错值，
/// 所以把它单独拎出来钉住。
#[inline]
pub const fn lapic_id_from_register(register: u32) -> u32 {
    register >> 24
}

/// 经 **MMIO** 读本地 APIC 的标识。
///
/// **为什么它在实现层而不是 `boot`**：这是对**设备寄存器**的指针运算 + 易失读 ——
/// 让它留在中性层就是**跨层直连**（S14 / ADR-007）。中性层只说"要 APIC 标识"。
///
/// # Safety
/// 调用方必须保证 LAPIC 的 MMIO **已被映射**。**未映射时是取数故障（复位）**，
/// 不是返回错误 —— 无法把它变成 `Option` 而不撒谎。
#[cfg(target_os = "uefi")]
pub unsafe fn read_id_via_mmio() -> u32 {
    // SAFETY: 由调用方保证 MMIO 已映射（见函数文档）。
    let raw = unsafe { mmio_read32(LAPIC_DEFAULT_BASE + LAPIC_ID as u64) };
    lapic_id_from_register(raw)
}

/// 写一个 32 位 MMIO 寄存器。
///
/// # Safety
/// 同 [`mmio_read32`]。
#[cfg(target_os = "uefi")]
pub unsafe fn mmio_write32(address: u64, value: u32) {
    // SAFETY: 由调用方保证（见函数文档）。
    unsafe { core::ptr::write_volatile(address as *mut u32, value) };
}

/// 写一个 MSR。
///
/// # Safety
/// `index` 必须是**当前 CPU 上存在且可写**的 MSR —— 写不存在的 MSR 会 `#GP`。
#[cfg(target_os = "uefi")]
pub unsafe fn wrmsr(index: u32, value: u64) {
    // SAFETY: 由调用方保证 MSR 存在且可写（见函数文档）。
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") index,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// 执行一次由 [`icr_writes`] 产出的寄存器写入。
///
/// # Safety
/// 同 [`mmio_write32`] / [`wrmsr`]。
#[cfg(target_os = "uefi")]
unsafe fn apply_write(write: RegisterWrite) {
    match write {
        RegisterWrite::Mmio32 { address, value } => unsafe { mmio_write32(address, value) },
        RegisterWrite::Msr { index, value } => unsafe { wrmsr(index, value) },
    }
}

/// 读 ICR 的当前值（用于轮询投递状态）。
///
/// # Safety
/// 同 [`mmio_read32`] / [`rdmsr`]。
#[cfg(target_os = "uefi")]
unsafe fn read_icr(access: ApicAccess) -> u64 {
    match access {
        // xAPIC：投递状态在 ICR **低半**（ICR0）里。
        ApicAccess::Xapic { base } => unsafe { mmio_read32(base + LAPIC_ICR_LOW as u64) as u64 },
        ApicAccess::X2apic => unsafe { rdmsr(X2APIC_MSR_ICR) },
    }
}

/// 发一个 IPI 并**等到它被投递**（轮询 ICR 的投递状态位）。
///
/// 顺序由 [`icr_writes`] 保证（xAPIC 必须先写高半 —— 写低半才触发发送）。
///
/// # Safety
/// 同 [`apply_write`]；且 `access` 必须与固件当前实际模式一致（用 x2APIC 的 MSR 去访问
/// 一个 xAPIC 模式的 LAPIC 不会报错，只会**什么都不发生**）。
#[cfg(target_os = "uefi")]
pub unsafe fn send_ipi(access: ApicAccess, value: u64) {
    let sequence = icr_writes(access, value);
    unsafe { apply_write(sequence.first) };
    if let Some(second) = sequence.second {
        unsafe { apply_write(second) };
    }
    for _ in 0..ICR_WAIT_SPINS {
        if !icr_busy(unsafe { read_icr(access) }) {
            return;
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_xapic_revert_is_two_steps_and_not_a_direct_switch() {
        // x2APIC 直接切 xAPIC 是非法状态转换（#GP），参考实现也是两步
        // （common/sys/lapic.c:376-381）：先同时清 bit 10 与 bit 11，再单独置 bit 11。
        let on = with_x2apic_enabled(0);
        let (disabled, xapic) = xapic_revert_steps(on).expect("开着 x2APIC 就有得退");
        assert_eq!(disabled & APIC_BASE_X2APIC, 0, "第一步必须清掉 bit 10");
        assert_eq!(disabled & APIC_BASE_ENABLE, 0, "第一步连 bit 11 也清掉（APIC 全关）");
        assert_eq!(xapic & APIC_BASE_ENABLE, APIC_BASE_ENABLE, "第二步打开 xAPIC");
        assert_eq!(xapic & APIC_BASE_X2APIC, 0, "第二步不得把 bit 10 又置回来");
        // 最终值与单步计算一致，但**写入必须分两步**。
        assert_eq!(xapic, with_x2apic_disabled(on));
    }

    #[test]
    fn reverting_is_idempotent_when_x2apic_is_not_in_effect() {
        assert!(xapic_revert_steps(0).is_none(), "APIC 全关时没什么可退");
        assert!(
            xapic_revert_steps(APIC_BASE_ENABLE).is_none(),
            "只有 bit 11 时 x2APIC 并未生效，不该假装有东西可退"
        );
    }

    #[test]
    fn a_permanently_disabled_xapic_is_reported_rather_than_guessed() {
        // Meteor Lake 及以后可以永久关掉 xAPIC；那时回退不可能成功，
        // 必须**先查再写**（写坏 MSR 会 #GP），并如实失败而不是假装成功。
        assert!(xapic_permanently_disabled(true, ARCH_CAPS_XAPIC_DISABLE, 1));
        assert!(
            !xapic_permanently_disabled(false, ARCH_CAPS_XAPIC_DISABLE, 1),
            "CPU 不报告该能力时不得据此拒绝"
        );
        assert!(!xapic_permanently_disabled(true, 0, 1), "不支持该特性时状态位无意义");
        assert!(!xapic_permanently_disabled(true, ARCH_CAPS_XAPIC_DISABLE, 0), "没被永久关掉");
    }
    use super::*;

    #[test]
    fn apic_state_reads_the_two_documented_bits() {
        // **两个位都要看**—— 只看 bit 10 会把"固件把 APIC 整体关着、而 bit 10 残留为 1"
        // 误判成"已启用 x2APIC"，于是按 x2APIC 去访问会得到一个"看起来能读、实际无效"的结果。
        let both = ApicState::from_base(APIC_BASE_ENABLE | APIC_BASE_X2APIC | 0xFEE0_0000);
        assert!(both.global_enable, "bit 11 已置位");
        assert!(both.x2apic, "bit 11 与 bit 10 都置位才算启用 x2APIC");
        assert_eq!(both.base, APIC_BASE_ENABLE | APIC_BASE_X2APIC | 0xFEE0_0000, "原始值要原样保留");
        let only_bit10 = ApicState::from_base(APIC_BASE_X2APIC | 0xFEE0_0000);
        assert!(!only_bit10.global_enable);
        assert!(!only_bit10.x2apic, "bit 11 没置位时 bit 10 不生效");
        // 本机真机实测值：0xfee00900 = 基址 0xFEE00000 | 全局使能(0x800) | BSP 标志(0x100)。
        let real = ApicState::from_base(0xfee0_0900);
        assert!(real.global_enable);
        assert!(!real.x2apic, "本机是 xAPIC");
    }

    #[test]
    fn the_apic_id_is_the_top_byte_of_the_register() {
        // 【为什么值得单独测】移位数写错不会崩，只会给出一个**看起来像标识**的错值 ——
        // 而它会被拿去和 CPUID 的结果比对，于是"比对了但比的是错的东西"。
        assert_eq!(lapic_id_from_register(0x0A00_0000), 0x0A);
        assert_eq!(lapic_id_from_register(0x0000_0000), 0x00);
        assert_eq!(lapic_id_from_register(0xFF00_0000), 0xFF);
        // 低 24 位是**其他字段**（保留位/型号等）—— 绝不能被当成标识。
        assert_eq!(lapic_id_from_register(0x0000_FFFF), 0x00, "低 24 位不是标识");
    }

    #[test]
    fn init_ipi_places_the_destination_differently_per_mode() {
        // **这是 S3 最容易错的地方**：xAPIC 与 x2APIC 的目的地位置不同。
        // 混用不会报错，只会把 IPI 发给别的核 —— 真机上表现为静默复位。
        let xapic = icr_value(ApicMode::Xapic, ApicId(0x12), DeliveryMode::Init, 0, true)
            .expect("xAPIC 目的地 0x12 合法");
        assert_eq!((xapic >> ICR_XAPIC_DEST_SHIFT) & 0xFF, 0x12, "xAPIC 目的地在 bit 56");
        assert_eq!(
            (xapic >> ICR_DELIVERY_SHIFT) & ICR_DELIVERY_MASK,
            DeliveryMode::Init as u64,
            "投递模式在 bit 8"
        );
        assert_eq!(xapic & ICR_LEVEL_ASSERT, ICR_LEVEL_ASSERT, "INIT 需要 assert");
        // **这里我一开始把断言写错了**：我查的是 bits 32-63 全零，但 xAPIC 的目的地
        // 在 bit 56，**本来就落在 32-63 之内** —— 于是断言必然失败。
        // 真正要表达的是「xAPIC 不得**另外**把目的地放进 bit 32」，即 bits 32-55 为空。
        assert_eq!(
            (xapic >> ICR_X2APIC_DEST_SHIFT) & 0x00FF_FFFF,
            0,
            "xAPIC 的目的地只应出现在 bit 56-63，bits 32-55 必须为空"
        );

        let x2 = icr_value(ApicMode::X2apic, ApicId(0x1234_5678), DeliveryMode::Init, 0, true)
            .expect("x2APIC 目的地任意 32 位都合法");
        assert_eq!(
            (x2 >> ICR_X2APIC_DEST_SHIFT) & 0xFFFF_FFFF,
            0x1234_5678,
            "x2APIC 目的地在 bit 32"
        );
        // **第二处我写错的断言**（与上一处同类）：x2APIC 的目的地占 bits 32-63，
        // **本来就包含 bits 56-63**，所以"那里为零"是错的。改成断言**完整编码值**，
        // 不再靠我自己算位区间 —— 那样每算一次就多一次错的机会。
        assert_eq!(
            x2,
            (0x1234_5678u64 << ICR_X2APIC_DEST_SHIFT) | 0x4500,
            "x2APIC 的完整编码：目的地 << 32 | Init(5) << 8 | assert"
        );
    }

    #[test]
    fn startup_ipi_carries_the_vector_in_the_low_byte() {
        // SIPI 的向量就是 AP 入口页号（vector << 12），必须落在低 8 位。
        let sipi = icr_value(ApicMode::X2apic, ApicId(3), DeliveryMode::Startup, 0x08, true)
            .expect("合法");
        assert_eq!(sipi & ICR_VECTOR_MASK, 0x08, "SIPI 向量在低 8 位");
        assert_eq!((sipi >> ICR_DELIVERY_SHIFT) & ICR_DELIVERY_MASK, DeliveryMode::Startup as u64);
        assert_eq!(sipi & ICR_LEVEL_ASSERT, ICR_LEVEL_ASSERT);
        assert_eq!(sipi & ICR_TRIGGER_LEVEL, 0, "SIPI 不用电平触发");
    }

    #[test]
    fn a_deasserted_ipi_clears_the_assert_bit() {
        // INIT 之后要发一次 deassert（电平语义），否则后续 IPI 可能被忽略。
        let deassert = icr_value(ApicMode::X2apic, ApicId(1), DeliveryMode::Init, 0, false)
            .expect("合法");
        assert_eq!(deassert & ICR_LEVEL_ASSERT, 0, "deassert 必须清掉 assert 位");
        assert_eq!((deassert >> ICR_DELIVERY_SHIFT) & ICR_DELIVERY_MASK, DeliveryMode::Init as u64);
    }

    #[test]
    fn broadcast_reaches_everyone_but_self() {
        let all = icr_value(ApicMode::X2apic, ApicId::ALL_BUT_SELF, DeliveryMode::Init, 0, true)
            .expect("合法");
        assert_eq!((all >> ICR_X2APIC_DEST_SHIFT) & 0xFFFF_FFFF, 0xFFFF_FFFF);
    }

    #[test]
    fn xapic_writes_the_high_half_first_because_the_low_half_triggers() {
        // **顺序是语义**：写 ICR0（低半）才触发发送。先写低半的话，IPI 会带着上一次
        // 留在 ICR1 里的目的地发出去 —— 发给错误的核，真机上是静默复位。
        let value = (0x12u64 << ICR_XAPIC_DEST_SHIFT) | 0x4500;
        let seq = icr_writes(ApicAccess::Xapic { base: LAPIC_DEFAULT_BASE }, value);
        // **这里我又写错了一次断言**（第三次同类）：xAPIC 的 ICR1 里目的地在该寄存器的
        // **bits 24-31**，所以值应是 `0x12 << 24` 而**不是** `0x12`。
        // 实现把目的地放在 64 位视图的 bit 56，`>> 32` 之后正好落在 ICR1 的 bit 24 —— 是对的。
        assert_eq!(
            seq.first,
            RegisterWrite::Mmio32 {
                address: LAPIC_DEFAULT_BASE + 0x310,
                value: 0x12u32 << 24,
            },
            "必须先写高半 ICR1，目的地在其 bits 24-31"
        );
        assert_eq!(
            seq.second,
            Some(RegisterWrite::Mmio32 { address: LAPIC_DEFAULT_BASE + 0x300, value: 0x4500 }),
            "再写低半 ICR0，写它才触发发送"
        );
    }

    #[test]
    fn x2apic_writes_the_whole_value_in_one_msr_write() {
        let value = (0x1234_5678u64 << ICR_X2APIC_DEST_SHIFT) | 0x4500;
        let seq = icr_writes(ApicAccess::X2apic, value);
        assert_eq!(seq.first, RegisterWrite::Msr { index: X2APIC_MSR_ICR, value });
        assert_eq!(seq.second, None, "x2APIC 只需一次写入");
    }

    #[test]
    fn x2apic_counts_as_enabled_only_when_the_apic_is_globally_enabled_too() {
        // **只看 bit 10 是错的**：bit 11 关着时 bit 10 无效。固件留下"bit 10 残留为 1"
        // 是完全可能的，那时按 x2APIC 访问会得到"看起来能读、实际无效"的结果。
        assert!(!x2apic_enabled(0), "两个位都关 = 未启用");
        // **这里我写反了**：消息说「只开全局 = 仍是 xAPIC」，断言却要求它为 true ——
        // **断言与它自己的消息矛盾**。这类错不是位区间或大小算错，而是"写完没读一遍"。
        assert!(!x2apic_enabled(APIC_BASE_ENABLE), "只开全局 = 仍是 xAPIC");
        assert!(!x2apic_enabled(APIC_BASE_X2APIC), "**只置 bit 10 不算启用**");
        assert!(x2apic_enabled(APIC_BASE_ENABLE | APIC_BASE_X2APIC), "两位都置才算启用");
    }

    #[test]
    fn enabling_x2apic_also_opens_the_global_apic_gate() {
        // 只置 bit 10 而不开 bit 11，等于把开关放在一个关着的总闸后面。
        let enabled = with_x2apic_enabled(0);
        assert!(enabled & APIC_BASE_X2APIC != 0, "必须置 bit 10");
        assert!(enabled & APIC_BASE_ENABLE != 0, "**也必须置 bit 11**");
        assert!(x2apic_enabled(enabled));
    }

    #[test]
    fn disabling_x2apic_keeps_the_lapic_reachable() {
        // 退回 xAPIC 时 **LAPIC 仍要能通过 MMIO 访问**，所以 bit 11 必须留着。
        let disabled = with_x2apic_disabled(APIC_BASE_ENABLE | APIC_BASE_X2APIC);
        assert_eq!(disabled & APIC_BASE_X2APIC, 0, "必须清 bit 10");
        assert_ne!(disabled & APIC_BASE_ENABLE, 0, "**bit 11 必须保留**，否则 LAPIC 整体不可达");
        assert!(!x2apic_enabled(disabled));
    }

    #[test]
    fn the_mode_switches_round_trip() {
        let on = with_x2apic_enabled(APIC_BASE_ENABLE);
        assert!(x2apic_enabled(on));
        assert!(!x2apic_enabled(with_x2apic_disabled(on)));
    }

    #[test]
    fn the_access_mode_follows_the_kernel_not_the_firmware() {
        // 固件开了 x2APIC 但**内核不支持** -> 必须用 xAPIC（brxLimine 会把固件的
        // x2APIC 退回 xAPIC）。反过来，内核支持且固件已开 -> 用 x2APIC。
        assert_eq!(select_access(false, true), ApicAccess::Xapic { base: LAPIC_DEFAULT_BASE });
        assert_eq!(select_access(false, false), ApicAccess::Xapic { base: LAPIC_DEFAULT_BASE });
        assert_eq!(select_access(true, true), ApicAccess::X2apic);
        // 内核支持但固件没开 -> **不擅自打开**：那需要写 MSR 0x1B，属独立决定（S4）。
        assert_eq!(select_access(true, false), ApicAccess::Xapic { base: LAPIC_DEFAULT_BASE });
    }

    #[test]
    fn the_ipi_constants_are_exactly_what_the_encoder_produces() {
        // **把常量绑到编码器上**（S13 单点）—— 常量是手写的十六进制，
        // 而"手写魔数"正是本会话反复出错的地方。这里让编码器当裁判。
        // 目的地取 0：这两个常量只描述**投递模式 + assert**，与目的地无关。
        assert_eq!(
            IPI_INIT_ASSERT,
            icr_value(ApicMode::X2apic, ApicId(0), DeliveryMode::Init, 0, true).expect("合法"),
            "INIT assert 必须与编码器一致"
        );
        assert_eq!(
            IPI_INIT_DEASSERT,
            icr_value(ApicMode::X2apic, ApicId(0), DeliveryMode::Init, 0, false).expect("合法"),
            "INIT deassert 必须与编码器一致"
        );
        assert_eq!(
            IPI_SIPI_BASE,
            icr_value(ApicMode::X2apic, ApicId(0), DeliveryMode::Startup, 0, true).expect("合法"),
            "SIPI 基础值必须与编码器一致"
        );
    }

    #[test]
    fn the_delivery_status_bit_is_the_one_the_reference_polls() {
        // brxLimine `lapic.c:301` 轮询的是 `ICR0 & (1 << 12)`。
        assert_eq!(ICR_DELIVERY_STATUS, 1 << 12);
        assert!(icr_busy(1 << 12), "置位即投递中");
        assert!(!icr_busy(0), "清零即已送达");
        // 其他位不得被误判成投递中。
        assert!(!icr_busy(0x4500), "0x4500 不含 bit 12");
    }

    #[test]
    fn the_encoding_reproduces_brxlimines_exact_icr_values() {
        // **对照参考实现的硬证据。** brxLimine `common/sys/smp.c` 里写死了这两个值：
        // INIT assert = `0x4500`、SIPI = `vector | 0x4600`。
        // 把我们的编码与它们逐位对上，等于**用参考实现证明位布局没抄错**。
        let init = icr_value(ApicMode::X2apic, ApicId(0), DeliveryMode::Init, 0, true)
            .expect("合法");
        assert_eq!(init, 0x4500, "INIT assert 必须与 brxLimine 的 0x4500 一致");

        let sipi = icr_value(ApicMode::X2apic, ApicId(0), DeliveryMode::Startup, 0x08, true)
            .expect("合法");
        assert_eq!(sipi, 0x08 | 0x4600, "SIPI 必须与 brxLimine 的 vector|0x4600 一致");

        // deassert：同样的投递模式，只是清掉 assert 位。
        let deassert = icr_value(ApicMode::X2apic, ApicId(0), DeliveryMode::Init, 0, false)
            .expect("合法");
        assert_eq!(deassert, 0x0500, "deassert 应为 0x4500 去掉 bit 14");
    }

    #[test]
    fn an_xapic_destination_beyond_eight_bits_is_rejected_not_truncated() {
        // **截断是危险的**：0x1FF 截断成 0xFF 会把 IPI 发给另一个核，而调用方以为发对了。
        assert_eq!(
            icr_value(ApicMode::Xapic, ApicId(0x1FF), DeliveryMode::Init, 0, true),
            Err(ApicError::DestinationTooLargeForXapic)
        );
        // 边界：0xFF 仍然合法。
        assert!(icr_value(ApicMode::Xapic, ApicId(0xFF), DeliveryMode::Init, 0, true).is_ok());
    }
}
