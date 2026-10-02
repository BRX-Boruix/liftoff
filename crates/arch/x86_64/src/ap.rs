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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApTrampoline {
    /// HHDM 偏移。跳板用它把**物理**地址转成可访问的虚拟地址 ✓
    /// （参考实现里 `info_struct`、GDTR 都要经它换算 ✓）。
    pub hhdm: u64,
    /// AP 写 1、BSP 轮询 —— **启动成功的唯一证据** ✓。
    ///
    /// 参考实现用 `xchg` 原子写（`smp_trampoline.asm_x86:176-177`）✓；
    /// BSP 侧必须**易失读**，否则优化器可能把它提到循环外 ✗。
    pub booted_flag: u8,
    /// **进度标记**：跳板每走完一步就写一次 ✓。BSP 超时后读回它，于是
    /// "IPI 根本没送到"（还是 `0`）与"AP 跑了、停在跳板第 N 步"（停在 `N`）**一眼可分** ✗。
    ///
    /// 编码（**与 `global_asm!` 里的立即数必须一致** ✓，由 `asm_check.py` 钉住）：
    /// `1` 实模式已在执行、`2` 已进保护模式、`3` 已开分页、`4` 已在 64 位、
    /// `5` 已写 `booted_flag` 即将自旋。
    pub stage: u8,
    /// 显式补齐，使 `target_mode` 落在 4 字节边界。
    pub pad0: [u8; 2],
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
        stage: 0,
        pad0: [0; 2],
        target_mode: 0,
        cr3: 0,
        info_struct: 0,
        temp_stack_lo: 0,
        temp_stack_hi: 0,
        gdtr: 0,
        pad1: 0,
    };
}

/// 参数块里**必须放进 u32** 的字段 —— 哪一个放不下就报哪一个，不笼统说"地址太大"。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApField {
    /// 页表顶层物理地址。
    Cr3Top,
    /// 该 AP 的 `MpInfo` 物理地址。
    InfoStruct,
    /// 临时栈顶。
    TempStack,
    /// GDTR 线性地址。
    Gdtr,
}

/// 填写参数块时的失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApTrampolineError {
    /// 某个地址**超出 32 位**。
    ///
    /// **必须报错，绝不截断** ✗：跳板的参数帧里这些字段是 u32，截断会让 AP 跳到
    /// **错误的地方** —— 真机上表现为复位，而根因藏在几个数量级之外。
    AddressTooLarge(ApField),
}

impl core::fmt::Display for ApTrampolineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AddressTooLarge(field) => {
                write!(f, "地址超出 32 位，无法放进跳板参数帧：{field:?}")
            }
        }
    }
}

/// 填写参数块的输入（**已换算好的物理/线性地址**）。
#[derive(Clone, Copy, Debug)]
pub struct ApTrampolineInput {
    /// HHDM 偏移。
    pub hhdm: u64,
    /// 页表顶层**物理**地址。
    pub cr3_top: u64,
    /// 该 AP 的 `MpInfo` **物理**地址。
    pub info_struct: u64,
    /// 临时栈顶（完整 64 位地址）。
    pub temp_stack_top: u64,
    /// GDTR 的**线性**地址。
    pub gdtr: u64,
    /// 是否开启写保护（`CR0.WP`）。
    pub write_protect: bool,
}

/// 由输入**填写**参数块（纯逻辑、宿主可测）。
///
/// 把「地址 → lo/hi 对」这类换算**集中在一处** ✓ —— 散在汇编前的准备代码里最容易写错 ✗，
/// 而写错的表现是 AP 跳到错误地址（静默复位）。
///
/// **放不下的地址一律报错，不截断** ✓。
pub fn prepare(input: &ApTrampolineInput) -> Result<ApTrampoline, ApTrampolineError> {
    fn low32(value: u64, field: ApField) -> Result<u32, ApTrampolineError> {
        u32::try_from(value).map_err(|_| ApTrampolineError::AddressTooLarge(field))
    }
    let mut block = ApTrampoline::EMPTY;
    block.hhdm = input.hhdm;
    block.cr3 = low32(input.cr3_top, ApField::Cr3Top)?;
    block.info_struct = low32(input.info_struct, ApField::InfoStruct)?;
    // 栈是 64 位地址，**有意**拆成 lo/hi —— 高半放不下是正常的，不是错误 ✓。
    block.temp_stack_lo = input.temp_stack_top as u32;
    block.temp_stack_hi = (input.temp_stack_top >> 32) as u32;
    block.gdtr = low32(input.gdtr, ApField::Gdtr)?;
    if input.write_protect {
        block.target_mode |= 1 << 4;
    }
    // `booted_flag` 与 `stage` 保持 0：**只有 AP 自己**能推进它们 ✓ ——
    // 引导器预设任何一个，都会把"没发生"报成"发生了" ✗。
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::ApTrampoline;
    use core::mem::{offset_of, size_of};

    use super::{ApField, ApTrampolineError, ApTrampolineInput, prepare};

    pub(super) fn input() -> ApTrampolineInput {
        ApTrampolineInput {
            hhdm: 0xffff_8000_0000_0000,
            cr3_top: 0x1000,
            info_struct: 0x2000,
            temp_stack_top: 0xffff_8000_0003_0000,
            gdtr: 0x3000,
            write_protect: true,
        }
    }

    #[test]
    fn prepare_fills_every_field_and_splits_the_stack_address() {
        let block = prepare(&input()).expect("地址都放得下");
        assert_eq!(block.hhdm, 0xffff_8000_0000_0000);
        assert_eq!(block.cr3, 0x1000);
        assert_eq!(block.info_struct, 0x2000);
        assert_eq!(block.gdtr, 0x3000);
        // 栈是 64 位：**必须拆成 lo/hi** —— 32 位阶段存不下 ✓。
        assert_eq!(block.temp_stack_lo, 0x0003_0000);
        assert_eq!(block.temp_stack_hi, 0xffff_8000);
        assert_ne!(block.target_mode & (1 << 4), 0, "开了写保护");
        assert_eq!(block.booted_flag, 0, "**只有 AP 自己**能置位，引导器不得预设");
    }

    #[test]
    fn an_address_that_does_not_fit_is_rejected_not_truncated() {
        // **截断是危险的**：AP 会跳到错误地址，真机上是复位，根因藏在几个数量级之外。
        let mut bad = input();
        bad.cr3_top = 0x1_0000_0000;
        assert_eq!(
            prepare(&bad),
            Err(ApTrampolineError::AddressTooLarge(ApField::Cr3Top))
        );
        let mut bad = input();
        bad.info_struct = 0x1_0000_0000;
        assert_eq!(
            prepare(&bad),
            Err(ApTrampolineError::AddressTooLarge(ApField::InfoStruct))
        );
        let mut bad = input();
        bad.gdtr = 0x1_0000_0000;
        assert_eq!(prepare(&bad), Err(ApTrampolineError::AddressTooLarge(ApField::Gdtr)));
    }

    #[test]
    fn a_high_stack_address_is_normal_not_an_error() {
        // 栈**有意**是 64 位：高半放不下是正常的 ✓ —— 与"地址太大"是两回事。
        let mut high = input();
        high.temp_stack_top = 0xffff_ffff_ffff_f000;
        let block = prepare(&high).expect("栈地址高是正常的");
        assert_eq!(block.temp_stack_lo, 0xffff_f000);
        assert_eq!(block.temp_stack_hi, 0xffff_ffff);
    }

    #[test]
    fn write_protect_off_leaves_the_bit_clear() {
        let mut off = input();
        off.write_protect = false;
        assert_eq!(prepare(&off).expect("合法").target_mode & (1 << 4), 0);
    }

    #[test]
    fn the_parameter_block_layout_is_pinned() {
        // **跳板按固定偏移读它**：偏移写错不会报错，只会静默跑飞。逐个钉住。
        assert_eq!(offset_of!(ApTrampoline, hhdm), 0);
        assert_eq!(offset_of!(ApTrampoline, booted_flag), 8);
        assert_eq!(offset_of!(ApTrampoline, stage), 9, "进度标记在 offset 9");
        assert_eq!(offset_of!(ApTrampoline, pad0), 10);
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
    fn the_progress_marker_starts_at_zero_and_only_the_ap_may_advance_it() {
        // **为什么需要它**：AP 不醒时串口上只有一个 \`started=0\` ✗ —— 分不清
        // "IPI 根本没送到"与"AP 跑了、崩在第 N 步" ✓。跳板每走一步写一次这个字节，
        // BSP 超时后读回它，于是两者**一眼可分** ✓。
        assert_eq!(ApTrampoline::EMPTY.stage, 0, "初始必须是 0：还没开始");
        let block = prepare(&input()).expect("合法");
        assert_eq!(block.stage, 0, "**只有 AP 自己**能推进它，引导器不得预设");
        assert_eq!(block.booted_flag, 0);
    }

    #[test]
    fn an_empty_block_means_nothing_has_happened() {
        // 全零必须表达「未启动」—— 若初始值非零，BSP 会在 AP 真的起来之前就以为成功了。
        let block = ApTrampoline::EMPTY;
        assert_eq!(block.booted_flag, 0, "初始必须是「未启动」");
        assert_eq!(block.cr3, 0, "页表地址未填");
        assert_eq!(block.info_struct, 0, "MpInfo 地址未填");
    }
}

// ---- 以下**不是测试**：跳板的搬运与布局常量，`boot` 要真的用它们 ✗ ----
//
// 【缺陷修正】原先它们被夹在 `mod tests` **里面** ✗（`mod tests` 一直没闭合 ✓），
// 于是这些项**只在测试里存在** ✗ —— `boot` 无论怎么写都找不到它们 ✗。
// 这正是"没有调用方"的真实原因 ✓。

/// 跳板被搬到的低内存页**必须**在 1 MiB 以下。
///
/// **为什么**：AP 从**实模式**醒来，寻址只有 20 位 ✗ —— 跳板放高了，AP 根本取不到
/// 第一条指令（表现为"发了 IPI 但 AP 不醒"，而串口上什么都没有 ✗）。
pub const AP_LOW_LIMIT: u64 = 0x10_0000;

/// 一页（跳板 + 参数块 + GDTR 描述符都放在同一页里）。
pub const AP_PAGE_SIZE: usize = 4096;

/// 参数块在页内的**固定偏移**。
///
/// **为什么必须固定**：SIPI **只给向量、不给任何寄存器** ✗ —— 跳板无法被告知参数块在哪，
/// 只能按固定位置找 ✓。参考实现把 `passed_info` 放在跳板末尾的固定位置，同理 ✓。
pub const AP_FRAME_OFFSET: usize = 0x800;

/// GDTR 的 6 字节描述符在页内的**固定偏移**（紧跟在参数块之后）。
pub const AP_GDTR_OFFSET: usize = 0x880;

/// GDTR 描述符本身的字节数（limit(2) + base(4)）。
pub const AP_GDTR_BYTES: usize = 6;

/// 低页基址的**下界**。
///
/// **为什么不是 0**：引导器的页表**有意不映射第 0 页**（`bring_up` 里显式
/// `unmap(0, 0x1000)`，让空指针解引用变成故障而不是静默读写物理 0）✓。
/// 跳板一旦落在第 0 页，AP 在**打开分页的那一刻**取指就会 `#PF` ✗ ——
/// 症状又是"AP 不醒、串口上什么都没有"，最难查的那种 ✗。
pub const AP_LOW_FLOOR: u64 = 0x1000;

/// AP 唤醒位置（SIPI 向量）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SipiVector(pub u8);

/// 搬运跳板时的失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StageError {
    /// 基址不是页对齐。
    BaseUnaligned,
    /// 基址 + 一页越过 1 MiB —— AP 在实模式下取不到 ✗。
    AboveRealModeLimit,
    /// 目标缓冲放不下一页。
    BufferTooSmall,
    /// 跳板本身大于参数块偏移 —— 会**覆盖参数块** ✗（编译期就该发现，运行期再守一次）。
    TrampolineTooLarge,
    /// 基址落在**未映射**的低端（第 0 页）—— AP 一开分页就 `#PF` ✗。
    BelowMappedFloor,
    /// GDT 为空 —— `lgdt` 之后没有任何可用描述符，装载段寄存器立即 `#GP` ✗。
    GdtEmpty,
    /// GDT 与本页布局冲突（压住 GDTR 描述符，或越出本页）。
    GdtDoesNotFit,
}

impl core::fmt::Display for StageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BaseUnaligned => f.write_str("AP 跳板基址未页对齐"),
            Self::AboveRealModeLimit => f.write_str("AP 跳板基址加一页越过 1 MiB（实模式取不到）"),
            Self::BufferTooSmall => f.write_str("目标缓冲放不下一页"),
            Self::TrampolineTooLarge => f.write_str("跳板代码大于参数块偏移，会覆盖参数块"),
            Self::BelowMappedFloor => f.write_str("AP 跳板基址落在未映射的第 0 页"),
            Self::GdtEmpty => f.write_str("AP 跳板的 GDT 为空"),
            Self::GdtDoesNotFit => f.write_str("AP 跳板的 GDT 与页内布局冲突"),
        }
    }
}

/// 把跳板代码、GDT 与 GDTR 描述符写进**低内存页** —— 每个低页**只做一次** ✓，
/// 返回 **SIPI 向量**。
///
/// **纯逻辑 + 字节操作，宿主可测** ✓ —— 跳板字节由调用方给（真机从链接期符号取 ✓）。
///
/// **本函数是页内布局的唯一所有者** ✓（S13/S15）：跳板代码、GDT、GDTR 描述符全在这一页里，
/// 都由这里写，调用方**不再自己往页里写任何东西** ✗。
///
/// 【缺陷修正】上一版把 GDT 交给调用方写、而搬运函数又 `page.fill(0)` **整页清零** ✗ ——
/// 于是调用方刚写好的 GDT 被**清零** ✗。AP 拿到一张全零的 GDT，`lgdt` 之后装载
/// CS/DS 立即 `#GP` → 三重故障 ✗，而串口上只会看到"AP 不醒" ✗。
/// "两边各写一半"的分工本身就是缺陷，所以这里把所有权收回来 ✓。
///
/// **绝不在 AP 已启动之后再调** ✗：AP 在 `goto_address` 被内核设置之前**一直停在这一页
/// 的代码里**自旋（见本文件末尾的 `ap_mode64`）✓ —— 再清零或重写代码，已停下的 AP 会
/// **执行到 0** ✗。所以每个 AP 只调 `stage_frame` ✓。
///
/// 页内布局（**两侧共用同一组常量** ✓）：
///
/// ```text
/// +0x000  跳板代码（实模式入口，必须落在页首 —— SIPI 的 cs:ip 指向页首）
/// +0x800  参数块（ApTrampoline，40 字节）
/// +0x880  GDTR 的 6 字节描述符
/// +0x890  远指针槽（6 字节，**运行期**由跳板填绝对地址 ✓）
/// +0x900  GDT 副本（`spinup::build_gdt()`）
/// ```
pub fn install(
    trampoline: &[u8],
    gdt: &[u64],
    base: u64,
    low: &mut [u8],
) -> Result<SipiVector, StageError> {
    if base % AP_PAGE_SIZE as u64 != 0 {
        return Err(StageError::BaseUnaligned);
    }
    if base < AP_LOW_FLOOR {
        return Err(StageError::BelowMappedFloor);
    }
    if base + AP_PAGE_SIZE as u64 > AP_LOW_LIMIT {
        return Err(StageError::AboveRealModeLimit);
    }
    if low.len() < AP_PAGE_SIZE {
        return Err(StageError::BufferTooSmall);
    }
    if trampoline.len() > AP_FRAME_OFFSET {
        return Err(StageError::TrampolineTooLarge);
    }
    if gdt.is_empty() {
        return Err(StageError::GdtEmpty);
    }
    let gdt_bytes = gdt.len() * 8;
    // GDT 必须落在 GDTR 描述符**之后**，且不越出本页 ✓。
    if AP_GDTR_OFFSET + AP_GDTR_BYTES > AP_GDT_OFFSET || AP_GDT_OFFSET + gdt_bytes > AP_PAGE_SIZE {
        return Err(StageError::GdtDoesNotFit);
    }
    let page = &mut low[..AP_PAGE_SIZE];
    // 整页清零**只在这里**发生 ✓ —— 此时还没有任何 AP 停在页里 ✓。
    page.fill(0);
    page[..trampoline.len()].copy_from_slice(trampoline);
    for (index, word) in gdt.iter().enumerate() {
        let at = AP_GDT_OFFSET + index * 8;
        page[at..at + 8].copy_from_slice(&word.to_le_bytes());
    }
    // GDTR 描述符：limit(2) + base(4)。基址是**物理**地址 —— `lgdt` 在实模式下执行，
    // 那时还没有分页 ✓（`cs<<4` 得到的也是物理地址 ✓）。
    // 已校验 `base < 1 MiB`，故 `base + 0x900` 必然放进 u32（S19：先论证再截断）。
    let gdt_base = (base + AP_GDT_OFFSET as u64) as u32;
    let gdt_limit = (gdt_bytes - 1) as u16;
    page[AP_GDTR_OFFSET..AP_GDTR_OFFSET + 2].copy_from_slice(&gdt_limit.to_le_bytes());
    page[AP_GDTR_OFFSET + 2..AP_GDTR_OFFSET + AP_GDTR_BYTES]
        .copy_from_slice(&gdt_base.to_le_bytes());
    // SIPI 向量 = 基址 >> 12（必须落在低 8 位）。
    Ok(SipiVector((base >> 12) as u8))
}

/// 写**单个 AP** 的参数块 —— 每启动一个 AP 调一次 ✓。
///
/// **只动 `[AP_FRAME_OFFSET, AP_FRAME_OFFSET + 40)` 这 40 字节** ✗，代码与 GDT
/// **一个字节都不碰** ✓ —— 上一个 AP 很可能正停在这一页里自旋 ✓。
///
/// 先整块清零再逐字段写：结构体里有**显式 pad**（`pad0`/`pad1`），不先清零就会留着
/// **上一个 AP 的残留值** ✗。
///
/// 返回 `()` 而不是向量：向量由 `install` 决定，每个 AP 用的是**同一个** ✓
/// （AP 都从页首醒来，身份由参数块里的 `info_struct` 区分 ✓）。
pub fn stage_frame(frame: &ApTrampoline, low: &mut [u8]) -> Result<(), StageError> {
    if low.len() < AP_PAGE_SIZE {
        return Err(StageError::BufferTooSmall);
    }
    let end = AP_FRAME_OFFSET + core::mem::size_of::<ApTrampoline>();
    let page = &mut low[..AP_PAGE_SIZE];
    page[AP_FRAME_OFFSET..end].fill(0);
    // 逐字段写：**不依赖结构体在内存里的表示**（避免把 Rust 布局当协议 ✗）。
    let mut put = |off: usize, bytes: &[u8]| {
        let at = AP_FRAME_OFFSET + off;
        page[at..at + bytes.len()].copy_from_slice(bytes);
    };
    put(core::mem::offset_of!(ApTrampoline, hhdm), &frame.hhdm.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, booted_flag), &[frame.booted_flag]);
    put(core::mem::offset_of!(ApTrampoline, stage), &[frame.stage]);
    put(core::mem::offset_of!(ApTrampoline, target_mode), &frame.target_mode.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, cr3), &frame.cr3.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, info_struct), &frame.info_struct.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, temp_stack_lo), &frame.temp_stack_lo.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, temp_stack_hi), &frame.temp_stack_hi.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, gdtr), &frame.gdtr.to_le_bytes());
    Ok(())
}

// ===================== AP 跳板汇编（S5–S7） =====================
//
// **这一段必须留在生产代码里** ✗：上一版它被夹在 `#[cfg(test)] mod tests` 内部，
// 于是 UEFI 产物里根本不存在 —— 而 `cargo test` 与 UEFI 构建**双双退出 0**（假绿，见台账第 312 轮）。
//
// 三个已知缺陷（前两个上一版就死在这里，第三个我这次也踩了一次）：
// 1. **语法**：上一版是 NASM 语法 ✗。`global_asm!` 用 LLVM 集成汇编器 ✓，
//    且**在 x86 上默认 Intel 语法** ✓（与 `spinup.rs` 一致 ✓）—— 写 AT&T（`%eax`）会报
//    "unknown token in expression" ✗。
// 2. **语义**：实模式远跳转的操作数**恰好 6 字节**（4 偏移 + 2 选择子 ✓）。
//    上一版写成 `dd 0` + `dd 0x18` = 8 字节 ✗，选择子会从错误位置取 ✓。
// 3. **标签差值**：`asm_check.py` 记录过一个真实故障 —— 把标签差值写成裸标签，
//    汇编器把它当成 **RIP 相对内存读取** ✗。所以地址一律经 `lea` 或 `add` 立即数 ✓。

/// **运行期填入的远指针槽**（4 字节偏移 + 2 字节选择子）。
///
/// **为什么需要它**：选择子 `0x18` 指向的是 **flat 段（基址 0）** ✓ —— 那是给 BSP 的
/// `spinup` 用的同一套 GDT。所以 `jmp far 0x18:0x24` 会跳到**线性地址 0x24**，而不是
/// "页基址 + 0x24" ✗ —— AP 一进保护模式就取指故障 ✗。
/// 第 111 轮真机实测正是如此：跳板进度停在 `1`（实模式已执行 ✓、保护模式没到 ✗）。
/// 参考实现同样在**运行期**把 `页基址 + 段内偏移` 写进这样一个槽再跳 ✓
/// （`common/sys/smp_trampoline.asm_x86:16-21` ✓）。
pub const AP_FARPTR_OFFSET: usize = 0x890;
/// 远指针槽的字节数（4 字节偏移 + 2 字节选择子）。
pub const AP_FARPTR_BYTES: usize = 6;

/// GDT 在本页内的偏移。
///
/// **GDT 必须在低内存**：进保护模式时还没有分页 ✗。它由 `install` 写 ——
/// 页内布局只有一个所有者 ✓（S13/S15），不再由调用方各写一半 ✗。
pub const AP_GDT_OFFSET: usize = 0x900;

// 页内布局的**编译期**不变量 ✓：改常量时立刻失败，而不是等到真机上"AP 不醒" ✗。
const _: () = assert!(AP_FRAME_OFFSET + core::mem::size_of::<ApTrampoline>() <= AP_GDTR_OFFSET);
const _: () = assert!(AP_GDTR_OFFSET + AP_GDTR_BYTES <= AP_GDT_OFFSET);
const _: () = assert!(AP_GDT_OFFSET + 9 * 8 <= AP_PAGE_SIZE);
const _: () = assert!(AP_GDTR_OFFSET + AP_GDTR_BYTES <= AP_FARPTR_OFFSET);
const _: () = assert!(AP_FARPTR_OFFSET + AP_FARPTR_BYTES <= AP_GDT_OFFSET);

#[cfg(target_os = "uefi")]
core::arch::global_asm!(
    ".section .text",
    ".global ap_trampoline_start",
    ".global ap_trampoline_end",
    ".set AP_MODE32_OFF, ap_mode32 - ap_trampoline_start",
    ".set AP_MODE64_OFF, ap_mode64 - ap_trampoline_start",
    // ---- 页内布局：**参数块基址只出现一次** ✓，各字段都相对它表达 ✓ ----
    //
    // 【真机缺陷】上一版把参数块里的字段写成**裸偏移**（`[rbx + 8]`）✗，而同一条汇编里
    // `esp`/`cr3` 那两条又带了 `0x800` ✓ —— **两种口径混用** ✗。后果：`info_struct` 读成
    // 页内偏移 20 处的垃圾 → AP 取到错误的 `MpInfo` → 三重故障 → **整机复位循环** ✗；
    // `booted_flag` 也写到错误位置 ✗。所以这里让 `AP_FRAME` 只写一次，字段一律相对它 ✓。
    ".set AP_FRAME, 0x800",
    // 下面每个数字都必须等于 `ApTrampoline` 里对应字段的偏移 ✓（宿主测试钉住结构那一侧 ✓）。
    ".set AP_F_HHDM, AP_FRAME + 0",
    ".set AP_F_BOOTED, AP_FRAME + 8",
    ".set AP_F_STAGE, AP_FRAME + 9",
    ".set AP_F_CR3, AP_FRAME + 16",
    ".set AP_F_INFO, AP_FRAME + 20",
    ".set AP_F_STACK_LO, AP_FRAME + 24",
    ".set AP_GDTR, AP_FRAME + 0x80",
    // 远指针槽：在 GDTR 描述符之后、GDT 之前 ✓（编译期断言守住不重叠 ✓）。
    ".set AP_FARPTR, AP_FRAME + 0x90",
    ".code16",
    "ap_trampoline_start:",
    "    cli",
    "    xor ebx, ebx",
    "    mov bx, cs",
    "    shl ebx, 4",
    // 进度标记 1：实模式已经在执行 —— 这一条被写出来就说明 **IPI 送到了、CS 也对** ✓。
    "    mov byte ptr [ebx + AP_F_STAGE], 1",
    "    lgdt [ebx + AP_GDTR]",
    "    mov eax, cr0",
    "    or eax, 1",
    "    mov cr0, eax",
    // **远跳的偏移必须是绝对线性地址** ✓（选择子 0x18 是 flat 段，基址 0 ✗）。
    // 所以运行期把"页基址 + 段内偏移"写进槽里，再 `jmp far` 读它 ✓ ——
    // 与参考实现同一手法（`smp_trampoline.asm_x86:16-21` ✓）。
    // 编码写成显式字节：`67 66 FF /5` = addr32 + opsize32 + `jmp far m16:32 [ebx+disp32]`，
    // 免得汇编器按 16 位模式给出 `m16:16`（那样只会读走 4 个字节 ✗）。
    "    lea eax, [ebx + AP_MODE32_OFF]",
    "    mov [ebx + AP_FARPTR], eax",
    "    mov dword ptr [ebx + AP_FARPTR + 4], 0x18",
    "    .byte 0x67, 0x66, 0xff, 0xab",
    "    .long AP_FARPTR",
    ".code32",
    "ap_mode32:",
    // 进度标记 2：远跳进了保护模式。
    "    mov byte ptr [ebx + AP_F_STAGE], 2",
    "    mov ax, 0x20",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov ss, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    mov esp, [ebx + AP_F_STACK_LO]",
    "    mov eax, [ebx + AP_F_CR3]",
    "    mov cr3, eax",
    "    mov eax, cr4",
    "    or eax, 0x20",
    "    mov cr4, eax",
    "    mov ecx, 0xc0000080",
    "    rdmsr",
    "    or eax, 0x100",
    "    or eax, 0x800",
    "    wrmsr",
    "    mov eax, cr0",
    "    or eax, 0x80000000",
    "    mov cr0, eax",
    // 进度标记 3：分页已开（用的是我们自己的页表 ✓）。
    "    mov byte ptr [ebx + AP_F_STAGE], 3",
    "    lea eax, [ebx + AP_MODE64_OFF]",
    "    push 0x28",
    "    push eax",
    "    retf",
    ".code64",
    "ap_mode64:",
    // 进度标记 4：已经在 64 位模式里执行。
    "    mov byte ptr [ebx + AP_F_STAGE], 4",
    "    mov ax, 0x30",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov ss, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    mov eax, 1",
    "    xchg [rbx + AP_F_BOOTED], eax",
    // 进度标记 5：`booted_flag` 已写，即将自旋等内核。
    "    mov byte ptr [rbx + AP_F_STAGE], 5",
    "    mov edi, [rbx + AP_F_INFO]",
    "    mov rax, [rbx + AP_F_HHDM]",
    "    add rdi, rax",
    "1:",
    "    mov rax, [rdi + 16]",
    "    test rax, rax",
    "    jnz 2f",
    "    pause",
    "    jmp 1b",
    "2:",
    "    mov rbx, cr3",
    "    mov cr3, rbx",
    "    mov rsp, [rdi + 8]",
    "    push 0x30",
    "    push rsp",
    "    push 0x2",
    "    push 0x28",
    "    push rax",
    "    iretq",
    "ap_trampoline_end:",
);

/// 跳板字节（**只在 UEFI 目标上存在** —— 宿主测试用合成字节测 `install()` ✓）。
#[cfg(target_os = "uefi")]
pub fn trampoline_bytes() -> &'static [u8] {
    unsafe extern "C" {
        static ap_trampoline_start: u8;
        static ap_trampoline_end: u8;
    }
    // SAFETY: 两个符号由上面的 `global_asm!` 定义，且 start < end ✓。
    let start = &raw const ap_trampoline_start as *const u8;
    let end = &raw const ap_trampoline_end as *const u8;
    let len = (end as usize).wrapping_sub(start as usize);
    // SAFETY: 同一段只读代码 ✓。
    unsafe { core::slice::from_raw_parts(start, len) }
}
#[cfg(test)]
mod staging_tests {
    use super::tests::input;
    use super::{
        AP_FRAME_OFFSET, AP_GDT_OFFSET, AP_GDTR_BYTES, AP_GDTR_OFFSET, AP_LOW_FLOOR, AP_PAGE_SIZE,
        ApTrampoline, StageError, install, prepare, stage_frame,
    };

    /// 参数块的**期望字节**。
    ///
    /// **独立写一遍**，不调用被测代码 —— 否则测试只是把实现抄了一遍，永远为真 ✗。
    fn expected_frame_bytes(f: &ApTrampoline) -> [u8; 40] {
        let mut out = [0u8; 40];
        out[core::mem::offset_of!(ApTrampoline, hhdm)..][..8]
            .copy_from_slice(&f.hhdm.to_le_bytes());
        out[core::mem::offset_of!(ApTrampoline, booted_flag)] = f.booted_flag;
        out[core::mem::offset_of!(ApTrampoline, target_mode)..][..4]
            .copy_from_slice(&f.target_mode.to_le_bytes());
        out[core::mem::offset_of!(ApTrampoline, cr3)..][..4]
            .copy_from_slice(&f.cr3.to_le_bytes());
        out[core::mem::offset_of!(ApTrampoline, info_struct)..][..4]
            .copy_from_slice(&f.info_struct.to_le_bytes());
        out[core::mem::offset_of!(ApTrampoline, temp_stack_lo)..][..4]
            .copy_from_slice(&f.temp_stack_lo.to_le_bytes());
        out[core::mem::offset_of!(ApTrampoline, temp_stack_hi)..][..4]
            .copy_from_slice(&f.temp_stack_hi.to_le_bytes());
        out[core::mem::offset_of!(ApTrampoline, gdtr)..][..4]
            .copy_from_slice(&f.gdtr.to_le_bytes());
        out
    }

    /// 9 个描述符，每个都非零且互不相同 —— 这样"某个描述符没被写"会被看出来 ✓。
    fn gdt() -> [u64; 9] {
        let mut out = [0u64; 9];
        for (index, word) in out.iter_mut().enumerate() {
            *word = 0x1000_0000_0000_0000 | ((index as u64) << 8) | 0x92;
        }
        out
    }

    #[test]
    fn install_puts_the_trampoline_at_the_page_start_and_stage_frame_at_its_fixed_offset() {
        // **SIPI 只给向量**：AP 从 `vector<<12` 的**页首**开始执行 ✓，而参数块必须能被
        // 按**固定偏移**找到 ✓ —— 所以两者在同一页里的位置都是契约。
        let trampoline = [0xAAu8; 16];
        let frame = prepare(&input()).expect("合法");
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        let vector = install(&trampoline, &gdt(), 0x8_0000, &mut low).expect("应当成功");
        assert_eq!(vector.0, 0x80, "SIPI 向量 = 基址 >> 12");
        assert_eq!(&low[..16], &trampoline[..], "跳板必须在**页首**");
        stage_frame(&frame, &mut low).expect("应当成功");
        assert_eq!(
            &low[AP_FRAME_OFFSET..AP_FRAME_OFFSET + 40],
            &expected_frame_bytes(&frame)[..],
            "参数块必须**逐字节**等于期望值（含显式 pad）"
        );
        assert_eq!(low[AP_FRAME_OFFSET + 8], 0, "booted_flag 必须从 0 开始");
    }

    #[test]
    fn install_writes_the_gdt_and_a_descriptor_that_points_at_it() {
        // 【回归】上一版把 GDT 交给调用方写、而搬运函数**整页清零** ✗ ——
        // GDT 被清零，AP 装载段寄存器立即 #GP → 三重故障，症状是"AP 不醒" ✗。
        // 所以这里同时钉两件事：**GDT 的字节**在，且 **GDTR 的基址**指向它 ✓。
        let gdt = gdt();
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        install(&[0x90u8; 8], &gdt, 0x8_0000, &mut low).expect("应当成功");
        for (index, word) in gdt.iter().enumerate() {
            let at = AP_GDT_OFFSET + index * 8;
            assert_eq!(
                u64::from_le_bytes(low[at..at + 8].try_into().expect("8 字节")),
                *word,
                "第 {index} 个描述符必须原样落在页内"
            );
        }
        // GDTR：limit = 字节数 - 1，base = 物理基址 + GDT 偏移。
        assert_eq!(
            u16::from_le_bytes(low[AP_GDTR_OFFSET..AP_GDTR_OFFSET + 2].try_into().unwrap()),
            (gdt.len() * 8 - 1) as u16
        );
        assert_eq!(
            u32::from_le_bytes(
                low[AP_GDTR_OFFSET + 2..AP_GDTR_OFFSET + AP_GDTR_BYTES]
                    .try_into()
                    .unwrap()
            ),
            (0x8_0000 + AP_GDT_OFFSET) as u32,
            "GDTR 的基址必须指向**本页里的 GDT 副本**"
        );
    }

    #[test]
    fn stage_frame_writes_the_progress_marker() {
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        let mut block = prepare(&input()).expect("合法");
        block.stage = 3;
        stage_frame(&block, &mut low).expect("应当成功");
        assert_eq!(low[AP_FRAME_OFFSET + 9], 3, "进度标记必须落在 offset 9");
    }

    #[test]
    fn stage_frame_touches_only_the_parameter_block() {
        // 【回归】AP 启动后会**停在这一页的代码里**自旋（直到内核设置 goto_address）✓。
        // 于是给**下一个** AP 写参数块时，绝不能碰代码或 GDT ✗ —— 否则上一个 AP
        // 会执行到 0 ✗。这个测试就是钉这条不变量。
        let trampoline = [0xCCu8; 32];
        let gdt = gdt();
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        install(&trampoline, &gdt, 0x8_0000, &mut low).expect("应当成功");
        let code_before = low[..AP_FRAME_OFFSET].to_vec();
        let gdt_before = low[AP_GDT_OFFSET..AP_GDT_OFFSET + gdt.len() * 8].to_vec();

        let mut second = input();
        second.info_struct = 0x9_0000;
        let frame = prepare(&second).expect("合法");
        stage_frame(&frame, &mut low).expect("应当成功");

        assert_eq!(low[..AP_FRAME_OFFSET].to_vec(), code_before, "代码一个字节都不能动");
        assert_eq!(
            low[AP_GDT_OFFSET..AP_GDT_OFFSET + gdt.len() * 8].to_vec(),
            gdt_before,
            "GDT 一个字节都不能动"
        );
        assert_eq!(&low[AP_FRAME_OFFSET..AP_FRAME_OFFSET + 40], &expected_frame_bytes(&frame)[..]);
    }

    #[test]
    fn stage_frame_leaves_no_stale_bytes_from_the_previous_ap() {
        // 结构体里有**显式 pad**：不先清零就会留着上一个 AP 的值 ✗。
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        low[AP_FRAME_OFFSET..AP_FRAME_OFFSET + 40].fill(0xFF);
        let frame = prepare(&input()).expect("合法");
        stage_frame(&frame, &mut low).expect("应当成功");
        assert_eq!(
            &low[AP_FRAME_OFFSET..AP_FRAME_OFFSET + 40],
            &expected_frame_bytes(&frame)[..],
            "整块 40 字节都必须被重写，不留 0xFF"
        );
    }

    #[test]
    fn install_refuses_a_base_in_the_unmapped_first_page() {
        // 第 0 页**有意不映射**（空指针解引用要变成故障）✓ —— 跳板放那里，
        // AP 一开分页就 #PF ✗。
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        assert_eq!(
            install(&[0u8; 8], &gdt(), 0, &mut low),
            Err(StageError::BelowMappedFloor)
        );
        assert!(
            install(&[0u8; 8], &gdt(), AP_LOW_FLOOR, &mut low).is_ok(),
            "恰好在下界上必须接受"
        );
    }

    #[test]
    fn install_refuses_an_address_the_ap_could_not_reach_in_real_mode() {
        // 实模式只有 20 位寻址：基址 + 一页越过 1 MiB，AP 取不到第一条指令 ✗
        // —— 而症状是"发了 IPI 但 AP 不醒、串口上什么都没有"，最难查的那种 ✗。
        let gdt = gdt();
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        // **这里我又把边界算错过一次**：一页结束在**正好 1 MiB** 是合法的 ✓ ——
        // 它的最后一个字节是 `0xFFFFF`，仍在 20 位寻址范围内 ✓。
        // 所以 `0xF_F000` 必须**接受**，而起点在 1 MiB 的才拒绝。
        assert!(
            install(&[0u8; 8], &gdt, 0xF_F000, &mut low).is_ok(),
            "结束在正好 1 MiB 是合法的：最后一个字节是 0xFFFFF"
        );
        assert_eq!(
            install(&[0u8; 8], &gdt, 0x10_0000, &mut low),
            Err(StageError::AboveRealModeLimit),
            "起点在 1 MiB 就已经取不到"
        );
        assert_eq!(
            install(&[0u8; 8], &gdt, 0x8_0001, &mut low),
            Err(StageError::BaseUnaligned)
        );
    }

    #[test]
    fn install_refuses_a_trampoline_that_would_overflow_into_the_frame() {
        // 跳板若大于参数块偏移，就会**覆盖参数块** ✗ —— 编译期由下面的断言先拦住，
        // 运行期这里再守一次（链接器布局是外部输入，不能只靠"应该不会"✓）。
        let too_big = std::vec![0u8; AP_FRAME_OFFSET + 1];
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        assert_eq!(
            install(&too_big, &gdt(), 0x8_0000, &mut low),
            Err(StageError::TrampolineTooLarge)
        );
        let mut tiny = std::vec![0u8; 8];
        assert_eq!(
            install(&[], &gdt(), 0x8_0000, &mut tiny),
            Err(StageError::BufferTooSmall)
        );
    }

    #[test]
    fn install_refuses_an_empty_or_oversized_gdt() {
        // 空 GDT：`lgdt` 之后没有任何可用描述符，装载 CS/DS 立即 #GP ✗。
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        assert_eq!(install(&[0u8; 8], &[], 0x8_0000, &mut low), Err(StageError::GdtEmpty));
        // 大到越出本页：宁可报错，绝不静默截断 ✗（300 个描述符 = 2400 字节 > 页尾）。
        let huge = std::vec![0u64; 300];
        assert_eq!(
            install(&[0u8; 8], &huge, 0x8_0000, &mut low),
            Err(StageError::GdtDoesNotFit)
        );
    }

    #[test]
    fn the_page_layout_leaves_room_for_the_trampoline() {
        // 页内四段**依次不重叠**：代码 < 参数块 < GDTR 描述符 < GDT。
        assert!(AP_FRAME_OFFSET + 40 <= AP_GDTR_OFFSET, "参数块(40B)不得压到 GDTR 描述符");
        assert!(AP_GDTR_OFFSET + AP_GDTR_BYTES <= AP_GDT_OFFSET, "GDTR 描述符不得压到 GDT");
        assert!(AP_GDT_OFFSET + 9 * 8 <= AP_PAGE_SIZE, "GDT(9 描述符)必须在一页之内");
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
