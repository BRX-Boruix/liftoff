//! AP 启动跳板的**参数块**（S5/S6/S7 的宿主可测部分）。
//!
//! **为什么单独建模**：跳板是**汇编**，它按**固定偏移**读这个结构 —— 偏移写错**不会报错**，
//! 只会**静默跑飞** ✗（真机上表现为复位，极难定位）。所以布局由 `offset_of!` 断言钉住，
//! 与 `MpInfo` / `MpResponse` 同一手法。
//!
//! **只放跳板真正要读的字段**，不照抄 brxLimine 的 `trampoline_passed_info` ——
//! 那个还含 MTRR 恢复、`lapic_setup` 等**我们不需要**的东西 ✗，照抄只会带进无用复杂度。
//!
//! **`MpInfo.reserved` 不在这里** ✓：那是**内核**填的 AP 栈（见 `limine::mp` 的字段文档）✓。

/// AP 跳板参数块。
///
/// 字段顺序**有意**让 8 字节字段在最前、随后是 4 字节组、末尾显式补齐 ——
/// 于是**没有隐式填充**，汇编看到的就是这里写的样子 ✓。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ApTrampoline {
    /// HHDM 偏移。跳板用它把**物理**地址转成可访问的虚拟地址 ✓
    /// （参考实现里 `info_struct`、GDTR 都要经它换算 ✓）。
    pub hhdm: u64,
    /// AP 写 1、BSP 轮询 —— **启动成功的唯一证据** ✓。
    ///
    /// 参考实现用 `xchg` 原子写（`smp_trampoline.asm_x86:176-177`）✓；
    /// BSP 侧必须**易失读**，否则优化器可能把它提到循环外 ✗。
    pub booted_flag: u8,
    /// 显式补齐，使 `target_mode` 落在 4 字节边界。
    pub pad0: [u8; 3],
    /// 目标模式位。bit 4 = `CR0.WP`（写保护）—— 与参考实现同一编码 ✓。
    pub target_mode: u32,
    /// 我们的页表**顶层物理地址**。页表在低 4 GiB 内，故 u32 够用 ✓。
    pub cr3: u32,
    /// 该 AP 的 `MpInfo` **物理地址**（低 4 GiB 内）；跳板自行加 `hhdm` ✓。
    pub info_struct: u32,
    /// 临时栈顶的**低 32 位**。
    ///
    /// 64 位地址必须拆成 lo/hi 对：32 位阶段存不下 64 位指针 ✗ ——
    /// 这是 `SpinupArgs` 已经解决过的同一个问题 ✓。
    pub temp_stack_lo: u32,
    /// 临时栈顶的**高 32 位**。
    pub temp_stack_hi: u32,
    /// GDTR 的**线性地址**（低 4 GiB 内）✓。
    pub gdtr: u32,
    /// 显式尾部补齐：让 `size_of` 恰好等于 40，**不留隐式填充** ✓。
    pub pad1: u32,
}

impl ApTrampoline {
    /// 全零块 = 「**什么都没发生**」：`booted_flag = 0`（未启动）✓、其余待填 ✓。
    pub const EMPTY: Self = Self {
        hhdm: 0,
        booted_flag: 0,
        pad0: [0; 3],
        target_mode: 0,
        cr3: 0,
        info_struct: 0,
        temp_stack_lo: 0,
        temp_stack_hi: 0,
        gdtr: 0,
        pad1: 0,
    };
}

#[cfg(test)]
mod tests {
    use super::ApTrampoline;
    use core::mem::{offset_of, size_of};

    #[test]
    fn the_parameter_block_layout_is_pinned() {
        // **跳板按固定偏移读它**：偏移写错不会报错，只会静默跑飞。逐个钉住。
        assert_eq!(offset_of!(ApTrampoline, hhdm), 0);
        assert_eq!(offset_of!(ApTrampoline, booted_flag), 8);
        assert_eq!(offset_of!(ApTrampoline, pad0), 9);
        assert_eq!(offset_of!(ApTrampoline, target_mode), 12);
        assert_eq!(offset_of!(ApTrampoline, cr3), 16);
        assert_eq!(offset_of!(ApTrampoline, info_struct), 20);
        assert_eq!(offset_of!(ApTrampoline, temp_stack_lo), 24);
        assert_eq!(offset_of!(ApTrampoline, temp_stack_hi), 28);
        assert_eq!(offset_of!(ApTrampoline, gdtr), 32);
        assert_eq!(offset_of!(ApTrampoline, pad1), 36);
        assert_eq!(size_of::<ApTrampoline>(), 40, "必须是 40：不留隐式填充");
    }

    #[test]
    fn an_empty_block_means_nothing_has_happened() {
        // 全零必须表达「未启动」—— 若初始值非零，BSP 会在 AP 真的起来之前就以为成功了。
        let block = ApTrampoline::EMPTY;
        assert_eq!(block.booted_flag, 0, "初始必须是「未启动」");
        assert_eq!(block.cr3, 0, "页表地址未填");
        assert_eq!(block.info_struct, 0, "MpInfo 地址未填");
    }

    #[test]
    fn the_target_mode_bit_for_write_protect_matches_the_reference() {
        // 参考实现：`test dword [target_mode], (1 << 4)` 决定是否 `bts eax, 16`（CR0.WP）。
        // 所以 bit 4 的含义**不能改** —— 这是与参考实现共用的编码。
        const WP_BIT: u32 = 1 << 4;
        assert_eq!(WP_BIT, 16);
        let mut block = ApTrampoline::EMPTY;
        block.target_mode = WP_BIT;
        assert_ne!(block.target_mode & WP_BIT, 0, "bit 4 = WP");
    }
}
