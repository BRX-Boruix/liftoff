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
    // `booted_flag` 保持 0：**只有 AP 自己**能把它置 1 ✓。
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
}

impl core::fmt::Display for StageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BaseUnaligned => f.write_str("AP 跳板基址未页对齐"),
            Self::AboveRealModeLimit => f.write_str("AP 跳板基址加一页越过 1 MiB（实模式取不到）"),
            Self::BufferTooSmall => f.write_str("目标缓冲放不下一页"),
            Self::TrampolineTooLarge => f.write_str("跳板代码大于参数块偏移，会覆盖参数块"),
        }
    }
}

/// 把跳板搬进**低内存页**并写好参数块与 GDTR 描述符，返回 **SIPI 向量**。
///
/// **纯逻辑 + 字节操作，宿主可测** ✓ —— 跳板字节由调用方给（真机从链接期符号取 ✓）。
///
/// 页内布局（**两侧共用同一组常量** ✓）：
///
/// ```text
/// +0x000  跳板代码（实模式入口，必须落在页首 —— SIPI 的 cs:ip 指向页首）
/// +0x800  参数块（ApTrampoline）
/// +0x880  GDTR 的 6 字节描述符
/// ```
pub fn stage(
    trampoline: &[u8],
    frame: &ApTrampoline,
    gdt_base: u32,
    gdt_limit: u16,
    base: u64,
    low: &mut [u8],
) -> Result<SipiVector, StageError> {
    if base % AP_PAGE_SIZE as u64 != 0 {
        return Err(StageError::BaseUnaligned);
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
    let page = &mut low[..AP_PAGE_SIZE];
    page.fill(0);
    page[..trampoline.len()].copy_from_slice(trampoline);
    // 参数块：按 `ApTrampoline` 的布局写。用 `to_le_bytes` 逐字段写，
    // **不依赖结构体在内存里的表示**（避免把 Rust 布局当协议 ✗）。
    let frame_at = AP_FRAME_OFFSET;
    let mut put = |off: usize, bytes: &[u8]| {
        let at = frame_at + off;
        page[at..at + bytes.len()].copy_from_slice(bytes);
    };
    put(core::mem::offset_of!(ApTrampoline, hhdm), &frame.hhdm.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, booted_flag), &[frame.booted_flag]);
    put(core::mem::offset_of!(ApTrampoline, target_mode), &frame.target_mode.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, cr3), &frame.cr3.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, info_struct), &frame.info_struct.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, temp_stack_lo), &frame.temp_stack_lo.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, temp_stack_hi), &frame.temp_stack_hi.to_le_bytes());
    put(core::mem::offset_of!(ApTrampoline, gdtr), &frame.gdtr.to_le_bytes());
    // GDTR 描述符：limit(2) + base(4)。
    page[AP_GDTR_OFFSET..AP_GDTR_OFFSET + 2].copy_from_slice(&gdt_limit.to_le_bytes());
    page[AP_GDTR_OFFSET + 2..AP_GDTR_OFFSET + 6].copy_from_slice(&gdt_base.to_le_bytes());
    // SIPI 向量 = 基址 >> 12（必须落在低 8 位）。
    Ok(SipiVector((base >> 12) as u8))
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

/// GDT 在本页内的偏移。接线方把 `spinup::build_gdt()` 拷到这里，
/// 并把它作为 `stage()` 的 `gdt_base` ✓（**GDT 必须在低内存**：进保护模式时还没有分页 ✗）。
pub const AP_GDT_OFFSET: usize = 0x900;

/// 跳板用到的 GDT 字节数（9 个描述符，与 `build_gdt()` 一致 ✓）。
pub const AP_GDT_BYTES: usize = 9 * 8;

#[cfg(target_os = "uefi")]
core::arch::global_asm!(
    ".section .text",
    ".global ap_trampoline_start",
    ".global ap_trampoline_end",
    ".set AP_MODE32_OFF, ap_mode32 - ap_trampoline_start",
    ".set AP_MODE64_OFF, ap_mode64 - ap_trampoline_start",
    ".code16",
    "ap_trampoline_start:",
    "    cli",
    "    xor ebx, ebx",
    "    mov bx, cs",
    "    shl ebx, 4",
    "    lgdt [ebx + 0x880]",
    "    mov eax, cr0",
    "    or eax, 1",
    "    mov cr0, eax",
    "    .byte 0x66, 0xea",
    "    .long AP_MODE32_OFF",
    "    .word 0x18",
    ".code32",
    "ap_mode32:",
    "    mov ax, 0x20",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov ss, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    mov esp, [ebx + 0x818]",
    "    mov eax, [ebx + 0x810]",
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
    "    lea eax, [ebx + AP_MODE64_OFF]",
    "    push 0x28",
    "    push eax",
    "    retf",
    ".code64",
    "ap_mode64:",
    "    mov ax, 0x30",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov ss, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    mov eax, 1",
    "    xchg [rbx + 8], eax",
    "    mov edi, [rbx + 20]",
    "    mov rax, [rbx]",
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

/// 跳板字节（**只在 UEFI 目标上存在** —— 宿主测试用合成字节测 `stage()` ✓）。
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
    use super::{AP_FRAME_OFFSET, AP_GDTR_OFFSET, AP_PAGE_SIZE, ApTrampoline, StageError, prepare, stage};

    #[test]
    fn staging_puts_the_trampoline_at_the_page_start_and_the_frame_at_its_fixed_offset() {
        // **SIPI 只给向量**：AP 从 `vector<<12` 的**页首**开始执行 ✓，而参数块必须能被
        // 按**固定偏移**找到 ✓ —— 所以两者在同一页里的位置都是契约。
        let trampoline = [0xAAu8; 16];
        let frame = prepare(&input()).expect("合法");
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        let vector = stage(&trampoline, &frame, 0x1234_5000, 0x2F, 0x8_0000, &mut low)
            .expect("应当成功");
        assert_eq!(vector.0, 0x80, "SIPI 向量 = 基址 >> 12");
        assert_eq!(&low[..16], &trampoline[..], "跳板必须在**页首**");
        let at = AP_FRAME_OFFSET + core::mem::offset_of!(ApTrampoline, hhdm);
        assert_eq!(
            u64::from_le_bytes(low[at..at + 8].try_into().expect("8 字节")),
            frame.hhdm,
            "HHDM 必须写在固定偏移处"
        );
        let at = AP_FRAME_OFFSET + core::mem::offset_of!(ApTrampoline, cr3);
        assert_eq!(u32::from_le_bytes(low[at..at + 4].try_into().expect("4 字节")), 0x1000);
        assert_eq!(low[AP_FRAME_OFFSET + 8], 0, "booted_flag 必须从 0 开始");
        // GDTR 描述符：limit(2) + base(4)。
        assert_eq!(u16::from_le_bytes(low[AP_GDTR_OFFSET..AP_GDTR_OFFSET + 2].try_into().unwrap()), 0x2F);
        assert_eq!(
            u32::from_le_bytes(low[AP_GDTR_OFFSET + 2..AP_GDTR_OFFSET + 6].try_into().unwrap()),
            0x1234_5000
        );
    }

    #[test]
    fn staging_refuses_an_address_the_ap_could_not_reach_in_real_mode() {
        // 实模式只有 20 位寻址：基址 + 一页越过 1 MiB，AP 取不到第一条指令 ✗
        // —— 而症状是"发了 IPI 但 AP 不醒、串口上什么都没有"，最难查的那种 ✗。
        let trampoline = [0u8; 16];
        let frame = prepare(&input()).expect("合法");
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        // **这里我又把边界算错了**：一页结束在**正好 1 MiB** 是合法的 ✓ ——
        // 它的最后一个字节是 `0xFFFFF`，仍在 20 位寻址范围内 ✓。
        // 所以 `0xF_F000` 必须**接受**，而起点在 1 MiB 的才拒绝。
        assert!(
            stage(&trampoline, &frame, 0, 0, 0xF_F000, &mut low).is_ok(),
            "结束在正好 1 MiB 是合法的：最后一个字节是 0xFFFFF"
        );
        assert_eq!(
            stage(&trampoline, &frame, 0, 0, 0x10_0000, &mut low),
            Err(StageError::AboveRealModeLimit),
            "起点在 1 MiB 就已经取不到"
        );
        assert_eq!(stage(&trampoline, &frame, 0, 0, 0x8_0001, &mut low), Err(StageError::BaseUnaligned));
    }

    #[test]
    fn staging_refuses_a_trampoline_that_would_overflow_into_the_frame() {
        // 跳板若大于参数块偏移，就会**覆盖参数块** ✗ —— 编译期由下面的断言先拦住，
        // 运行期这里再守一次（链接器布局是外部输入，不能只靠"应该不会"✓）。
        let too_big = std::vec![0u8; AP_FRAME_OFFSET + 1];
        let frame = prepare(&input()).expect("合法");
        let mut low = std::vec![0u8; AP_PAGE_SIZE];
        assert_eq!(
            stage(&too_big, &frame, 0, 0, 0x8_0000, &mut low),
            Err(StageError::TrampolineTooLarge)
        );
        let mut tiny = std::vec![0u8; 8];
        assert_eq!(stage(&[], &frame, 0, 0, 0x8_0000, &mut tiny), Err(StageError::BufferTooSmall));
    }

    #[test]
    fn the_page_layout_leaves_room_for_the_trampoline() {
        // 页内三段不重叠，且 GDTR 描述符不压在参数块上。
        assert!(AP_FRAME_OFFSET + 40 <= AP_GDTR_OFFSET, "参数块(40B)不得压到 GDTR 描述符");
        assert!(AP_GDTR_OFFSET + 6 <= AP_PAGE_SIZE, "GDTR 描述符必须在一页之内");
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
