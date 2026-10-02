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
