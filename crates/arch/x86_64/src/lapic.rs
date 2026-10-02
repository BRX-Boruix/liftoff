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
}

impl core::fmt::Display for ApicError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DestinationTooLargeForXapic => {
                f.write_str("xAPIC 的目的地只有 8 位，超出即拒绝（截断会发给错误的核）")
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

/// `IA32_APIC_BASE` MSR（`0x1B`）—— xAPIC/x2APIC 的模式开关就在这里。
pub const IA32_APIC_BASE: u32 = 0x1B;

/// `IA32_APIC_BASE` bit 11：APIC **全局**使能。
pub const APIC_BASE_ENABLE: u64 = 1 << 11;

/// `IA32_APIC_BASE` bit 10：**x2APIC** 使能。
pub const APIC_BASE_X2APIC: u64 = 1 << 10;

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

#[cfg(test)]
mod tests {
    use super::*;

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
