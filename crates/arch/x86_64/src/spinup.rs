//! spinup 跳板：ExitBootServices 之后把机器状态重置成内核期望的样子，再进入内核。
//!
//! 结构对照 brxLimine（common/lib/spinup.asm_uefi_x86_64 + common/protos/limine_32.asm_x86）：
//!
//! 1. spinup_common64（64 位，留在映像原地址执行，不搬运）：
//!    cli -> lgdt（自建 GDT）-> lidt（空 IDT）-> retfq 重载 CS=0x28
//!    -> 数据段 =0x30 -> 切到低地址参数帧 -> retfq 到 32 位低地址 spinup_go32。
//! 2. spinup_go32（32 位，搬到低地址）：段=0x20 -> lldt 0 -> 关分页（CR0=0x11）
//!    -> EFER=0 -> CR4=0 -> CR3=0 -> 清 TSS busy 位并 ltr 0x38 -> call 到
//!    spinup_spinup32。
//! 3. spinup_spinup32（32 位起，搬到低地址）：PAT -> 可选 LA57 -> CR0.WP
//!    -> CR3=内核页表 -> PAE -> EFER.LME(+NX) -> CR0.PG -> 回到 64 位 -> 段=0x30
//!    -> 重载 GDT -> 跳到高半区 -> 按 base_revision 卸掉低半区 -> 构造 iretq 帧
//!    （入口、内核栈、全 GPR 清零）-> 进内核。
//!
//! 只搬 32 位部分：32 位 EIP 到不了 4 GiB 以上，所以 spinup_go32 /
//! spinup_spinup32 必须有一份低地址拷贝；64 位的 spinup_common64 留在映像里，
//! 用链接期符号直接 jmp（避免任何运行期重定位）。
//!
//! **两处 64 位续接点由参数帧提供绝对地址**（`mode64_low` / `hh_low`）：
//! 低地址拷贝里不能用「标签差值」算绝对地址 —— LLVM 会把 `mov ebx, 6f`
//! 汇编成 RIP 相对**内存读取**（实测字节 `8b 1d ...`），算出来是垃圾，
//! `retf` 会跳到未映射地址。

/// 段描述符编码（对照 brxLimine common/sys/gdt.s2.c）。
///
/// gran 是**完整第 6 字节**：[7]=G [6]=D/B [5]=L [4]=AVL [3:0]=limit 高位。
///
/// 因此它整体落在位 48..55（正好一个字节）；limit 的高 4 位被忽略，
/// limit 的高位以 gran 的低 4 位为准（与 brxLimine 的宏一致）。
pub const fn desc(limit: u32, base: u32, access: u8, gran: u8) -> u64 {
    let limit_low = (limit & 0xFFFF) as u64;
    let base_low = (base & 0xFFFF) as u64;
    let base_mid = ((base >> 16) & 0xFF) as u64;
    let base_hi = ((base >> 24) & 0xFF) as u64;
    limit_low
        | (base_low << 16)
        | (base_mid << 32)
        | ((access as u64) << 40)
        | ((gran as u64) << 48)
        | (base_hi << 56)
}

/// 9 项 GDT：0=null、1=32 位代码、2=32 位数据、3=32 位代码(G)、4=32 位数据(G)、
/// 5=64 位代码(L)、6=64 位数据、7=TSS 低、8=TSS 高。
///
/// 选择子：0x08 / 0x10 / 0x18 / 0x20 / 0x28 / 0x30 / 0x38。
pub const fn build_gdt() -> [u64; 9] {
    [
        0,
        desc(0xFFFF, 0, 0b1001_1011, 0b0000_0000),
        desc(0xFFFF, 0, 0b1001_0011, 0b0000_0000),
        desc(0xFFFF, 0, 0b1001_1011, 0b1100_1111),
        desc(0xFFFF, 0, 0b1001_0011, 0b1100_1111),
        desc(0, 0, 0b1001_1011, 0b0010_0000),
        desc(0, 0, 0b1001_0011, 0),
        desc(0, 0, 0x89, 0),
        0,
    ]
}

/// GDT 的静态实例：汇编里的 GDTR 用 .quad SPINUP_GDT 指向它。
#[repr(C, align(8))]
pub struct SpinupGdt(pub [u64; 9]);

/// 自建 GDT（选择子 0x08 到 0x38）。
#[unsafe(no_mangle)]
pub static SPINUP_GDT: SpinupGdt = SpinupGdt(build_gdt());

/// 32 位 trampoline 的参数帧：11 个 dword，arg0 在最低地址。
///
/// 汇编侧在 32 位模式下用 [esp + 4*i] 读 arg_i，64 位模式下用 [rsp + 4*i]
/// （两处 rsp/esp 都指向 arg0，见 layout 的说明）。
/// 其中 entry/stack/dmo 按 qword 读，所以 lo/hi 必须相邻且 lo 在前。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpinupArgs {
    /// 是否启用 5 级分页（我们不用，0）。
    pub level5pg: u32,
    /// 内核页表顶层物理地址（低 4 GiB 内）。
    pub pagemap_top: u32,
    /// 内核入口低 32 位。
    pub entry_lo: u32,
    /// 内核入口高 32 位。
    pub entry_hi: u32,
    /// 内核栈顶低 32 位。
    pub stack_lo: u32,
    /// 内核栈顶高 32 位。
    pub stack_hi: u32,
    /// GDTR 指针（由 stage_low_buffer 填，调用方不必知道）。
    pub gdt: u32,
    /// NX 可用（1）。
    pub nx_available: u32,
    /// direct map offset 低 32 位。
    pub dmo_lo: u32,
    /// direct map offset 高 32 位。
    pub dmo_hi: u32,
    /// base revision（大于等于 1 表示卸掉低半区）。
    pub base_revision: u32,
}

/// 低地址缓冲的布局结果（全部是低地址）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowBuffer {
    /// 32 位 spinup_go32 拷贝的入口。
    pub go32: usize,
    /// 32 位 spinup_spinup32 拷贝的入口。
    pub spinup32: usize,
    /// 参数帧地址（13 个 dword；`rsp` 指向它）。
    pub args: usize,
}

/// 4 GiB 上限：32 位 EIP 的硬约束。
pub const LOW_LIMIT: usize = 0x1_0000_0000;

/// 参数帧字节数：13 个 dword = 11 个参数 + `mode64_low` + `hh_low`。
///
/// 后两个槽由 Rust 填，用来告诉 32 位代码 64 位续接段的**绝对低地址**。
pub const ARG_FRAME_BYTES: usize = 52;

/// 低地址栈大小（参数帧**下方**，向下生长）。
pub const LOW_STACK_BYTES: usize = 4096;

/// 低地址缓冲的偏移布局（纯计算，宿主可测）。
///
/// 布局：`[go32][spinup32][stack][args]`，各段 16 字节对齐。
/// 参数帧放在最后：`rsp` 指向它，压栈向下长进 `stack` 区，不会踩到代码。
/// 返回 `(go32_off, spinup32_off, stack_off, args_off, total)`。
pub const fn layout(go32_len: usize, spinup32_len: usize) -> (usize, usize, usize, usize, usize) {
    const fn a16(v: usize) -> usize {
        (v + 15) & !15
    }
    let go32_off = 0usize;
    let spinup32_off = a16(go32_off + go32_len);
    let stack_off = a16(spinup32_off + spinup32_len);
    let args_off = a16(stack_off + LOW_STACK_BYTES);
    let total = args_off + ARG_FRAME_BYTES;
    (go32_off, spinup32_off, stack_off, args_off, total)
}
#[cfg(target_os = "uefi")]
core::arch::global_asm!(
    // 必须落在标准 .text 段：自定义段名会被 PE 工具截断并标成 DATA，
    // 固件可能因此把它映射成不可执行，跳进去就故障。
    ".section .text",
    ".global spinup_common64",
    ".global spinup_go32",
    ".global spinup_go32_end",
    ".global spinup_spinup32",
    ".global spinup_spinup32_mode64",
    ".global spinup_spinup32_hh",
    ".global spinup_spinup32_end",
    ".global spinup_gdtr",
    ".global spinup_idtr",
    // ---- 64 位入口：留在映像内原地址执行 ----
    ".code64",
    "spinup_common64:",
    "    cli",
    "    lgdt [rip + spinup_gdtr]",
    "    lidt [rip + spinup_idtr]",
    "    lea rbx, [rip + 1f]",
    "    push 0x28",
    "    push rbx",
    "    retfq",
    "1:",
    "    mov eax, 0x30",
    "    mov ds, eax",
    "    mov es, eax",
    "    mov fs, eax",
    "    mov gs, eax",
    "    mov ss, eax",
    // rdx = 参数帧低地址；rdi = spinup_go32 低地址拷贝；esi = spinup_spinup32 低地址。
    "    mov rsp, rdx",
    "    push 0x18",
    "    push rdi",
    "    retfq",
    // ---- 32 位：关分页、清 TSS ----
    ".code32",
    "spinup_go32:",
    "    mov eax, 0x20",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    mov ss, ax",
    "    xor eax, eax",
    "    lldt ax",
    "    mov eax, 0x00000011",
    "    mov cr0, eax",
    "    mov ecx, 0xc0000080",
    "    xor eax, eax",
    "    xor edx, edx",
    "    wrmsr",
    "    xor eax, eax",
    "    mov cr4, eax",
    "    mov cr3, eax",
    "    sub esp, 8",
    "    sgdt [esp]",
    "    mov eax, [esp + 2]",
    "    add esp, 8",
    "    mov byte ptr [eax + 0x3d], 0x89",
    "    mov ax, 0x38",
    "    ltr ax",
    "    call esi",
    "spinup_go32_end:",
    // ---- 32 位：重建分页并回到 64 位 ----
    // 进入时 esp 指向参数帧 arg0（call esi 之前）。
    "spinup_spinup32:",
    "    mov eax, 1",
    "    xor ecx, ecx",
    "    cpuid",
    "    test edx, 1 << 16",
    "    jz 2f",
    "    mov eax, 0x00070406",
    "    mov edx, 0x00000105",
    "    mov ecx, 0x277",
    "    wrmsr",
    "2:",
    "    cmp dword ptr [esp + 0], 0",
    "    je 3f",
    "    mov eax, cr4",
    "    bts eax, 12",
    "    mov cr4, eax",
    "3:",
    "    mov eax, cr0",
    "    bts eax, 16",
    "    mov cr0, eax",
    "    cld",
    "    mov eax, [esp + 4]",
    "    mov cr3, eax",
    "    mov eax, cr4",
    "    bts eax, 5",
    "    mov cr4, eax",
    "    mov ecx, 0xc0000080",
    "    xor edx, edx",
    "    mov eax, 1 << 8",
    "    cmp dword ptr [esp + 28], 0",
    "    je 4f",
    "    or eax, 1 << 11",
    "4:",
    "    wrmsr",
    "    mov eax, cr0",
    "    bts eax, 31",
    "    mov cr0, eax",
    // 32 -> 64：CS=0x28，EIP = mode64_low（参数帧槽 11，绝对地址由 Rust 填）。
    // 不用标签差值：`mov ebx, 6f` 会被汇编成 RIP 相对内存读取。
    "    mov ebx, [esp + 44]",
    "    push 0x28",
    "    push ebx",
    "    retf",
    ".code64",
    "spinup_spinup32_mode64:",
    // 此处 rsp 仍指向参数帧 arg0（retf 弹掉了 8 字节）。
    "    mov eax, 0x30",
    "    mov ds, eax",
    "    mov es, eax",
    "    mov fs, eax",
    "    mov gs, eax",
    "    mov ss, eax",
    "    mov eax, [rsp + 24]",
    "    lgdt [rax]",
    "    mov rax, [rsp + 32]",
    "    mov ebx, [rsp + 48]",
    "    add rbx, rax",
    "    add rsp, rax",
    "    jmp rbx",
    "spinup_spinup32_hh:",
    "    cmp dword ptr [rsp + 40], 1",
    "    jb 9f",
    "    mov rsi, cr3",
    "    lea rdi, [rsi + rax]",
    "    mov rcx, 256",
    "    xor rax, rax",
    "    rep stosq",
    "    mov cr3, rsi",
    "9:",
    "    mov rsi, [rsp + 16]",
    "    sub rsi, 8",
    "    mov qword ptr [rsi], 0",
    "    mov rax, [rsp + 8]",
    "    push 0x30",
    "    push rsi",
    "    push 0x2",
    "    push 0x28",
    "    push rax",
    "    xor eax, eax",
    "    xor ebx, ebx",
    "    xor ecx, ecx",
    "    xor edx, edx",
    "    xor esi, esi",
    "    xor edi, edi",
    "    xor ebp, ebp",
    "    xor r8d, r8d",
    "    xor r9d, r9d",
    "    xor r10d, r10d",
    "    xor r11d, r11d",
    "    xor r12d, r12d",
    "    xor r13d, r13d",
    "    xor r14d, r14d",
    "    xor r15d, r15d",
    "    iretq",
    "spinup_spinup32_end:",
    // ---- 数据：GDTR 指向映像内的 SPINUP_GDT；IDTR 为空表 ----
    ".code64",
    ".align 8",
    "spinup_gdtr:",
    "    .word 71",
    "    .quad SPINUP_GDT",
    "spinup_idtr:",
    "    .word 0",
    "    .quad 0",
);

#[cfg(target_os = "uefi")]
unsafe extern "C" {
    fn spinup_common64();
    static spinup_go32: u8;
    static spinup_go32_end: u8;
    static spinup_spinup32: u8;
    static spinup_spinup32_mode64: u8;
    static spinup_spinup32_hh: u8;
    static spinup_spinup32_end: u8;
    static spinup_gdtr: u8;
}
/// Exit 前调用：把 32 位跳板的两段、低地址栈、参数帧放进 buffer（必须小于 4 GiB）。
///
/// 64 位的 spinup_common64 不搬 —— 它留在映像里用链接期符号直接跳。
///
/// # Safety
///
/// buffer 必须指向 buffer_len 字节可写、且 Exit 后仍可读可执行的内存
/// （调用方用 EfiLoaderCode 分配）。
#[cfg(target_os = "uefi")]
pub unsafe fn stage_low_buffer(
    buffer: *mut u8,
    buffer_len: usize,
    args: &SpinupArgs,
) -> Option<LowBuffer> {
    let go32_src = &raw const spinup_go32 as usize;
    let go32_end = &raw const spinup_go32_end as usize;
    let sp32_src = &raw const spinup_spinup32 as usize;
    let sp32_end = &raw const spinup_spinup32_end as usize;
    let mode64_src = &raw const spinup_spinup32_mode64 as usize;
    let hh_src = &raw const spinup_spinup32_hh as usize;
    let go32_len = go32_end.checked_sub(go32_src)?;
    let sp32_len = sp32_end.checked_sub(sp32_src)?;
    let mode64_off = mode64_src.checked_sub(sp32_src)?;
    let hh_off = hh_src.checked_sub(sp32_src)?;
    let (go32_off, sp32_off, _stack_off, args_off, total) = layout(go32_len, sp32_len);
    if buffer_len < total {
        return None;
    }
    let base = buffer as usize;
    // 32 位 EIP 到不了 4 GiB 以上。
    if base.checked_add(total)? > LOW_LIMIT {
        return None;
    }
    let go32_low = base + go32_off;
    let sp32_low = base + sp32_off;
    let mode64_low = sp32_low.checked_add(mode64_off)?;
    let hh_low = sp32_low.checked_add(hh_off)?;
    // 参数帧里的 GDTR 指针必须指向映像内的 spinup_gdtr。
    let gdtr = &raw const spinup_gdtr as usize;
    if gdtr > u32::MAX as usize || mode64_low > u32::MAX as usize || hh_low > u32::MAX as usize {
        return None;
    }
    let words = [
        args.level5pg,
        args.pagemap_top,
        args.entry_lo,
        args.entry_hi,
        args.stack_lo,
        args.stack_hi,
        gdtr as u32,
        args.nx_available,
        args.dmo_lo,
        args.dmo_hi,
        args.base_revision,
        mode64_low as u32,
        hh_low as u32,
    ];
    // SAFETY: 调用方保证 buffer 可写；total 已校验不超过 buffer_len。
    unsafe {
        core::ptr::copy_nonoverlapping(go32_src as *const u8, buffer.add(go32_off), go32_len);
        core::ptr::copy_nonoverlapping(sp32_src as *const u8, buffer.add(sp32_off), sp32_len);
        for (index, word) in words.iter().enumerate() {
            core::ptr::write_unaligned(buffer.add(args_off + index * 4) as *mut u32, *word);
        }
    }
    Some(LowBuffer {
        go32: go32_low,
        spinup32: sp32_low,
        args: base + args_off,
    })
}

#[cfg(not(target_os = "uefi"))]
pub unsafe fn stage_low_buffer(
    _buffer: *mut u8,
    _buffer_len: usize,
    _args: &SpinupArgs,
) -> Option<LowBuffer> {
    None // 宿主没有裸机跳板；等价性由真机验证。
}

/// Exit 成功后调用：进跳板，不返回。
///
/// # Safety
///
/// 只能调用一次；low 必须来自 stage_low_buffer。
#[cfg(target_os = "uefi")]
pub unsafe fn spinup_go(low: LowBuffer) -> ! {
    // SAFETY: 三个值分别进 rdi/rsi/rdx（显式寄存器，不会与跳转目标混淆）；
    // 跳转目标是本 crate 汇编导出的链接期符号。
    unsafe {
        core::arch::asm!(
            "jmp {enter}",
            in("rdi") low.go32,
            in("rsi") low.spinup32,
            in("rdx") low.args,
            enter = sym spinup_common64,
            options(noreturn),
        )
    }
}

#[cfg(not(target_os = "uefi"))]
pub unsafe fn spinup_go(_low: LowBuffer) -> ! {
    unreachable!("spinup_go 只在 UEFI 目标上有意义")
}
#[cfg(test)]
mod gdt_tests {
    use super::{build_gdt, desc};

    /// 取第 index 个描述符的第 byte 个字节。
    fn byte_of(entry: u64, byte: u32) -> u8 {
        ((entry >> (byte * 8)) & 0xFF) as u8
    }

    /// gran 字节的位含义：[7]=G [6]=D/B [5]=L。
    fn flags(entry: u64) -> (bool, bool, bool) {
        let gran = byte_of(entry, 6);
        (gran & 0x80 != 0, gran & 0x40 != 0, gran & 0x20 != 0)
    }

    #[test]
    fn descriptor_fields_land_in_the_right_bytes() {
        let entry = desc(0xFFFF, 0, 0b1001_1011, 0b1100_1111);
        assert_eq!(byte_of(entry, 0), 0xFF, "limit 低字节");
        assert_eq!(byte_of(entry, 1), 0xFF, "limit 低字节");
        assert_eq!(byte_of(entry, 5), 0b1001_1011, "access 在第 5 字节");
        assert_eq!(byte_of(entry, 6), 0b1100_1111, "gran 是完整第 6 字节");
        assert_eq!(byte_of(entry, 7), 0x00, "第 7 字节只放 base 高位");
    }

    #[test]
    fn null_descriptors_are_zero() {
        assert_eq!(build_gdt()[0], 0);
        assert_eq!(build_gdt()[8], 0, "TSS 高位描述符为 0");
    }

    #[test]
    fn selector_0x18_is_a_32_bit_code_segment() {
        let (g, d, l) = flags(build_gdt()[3]);
        assert!(g, "G=1（4 KiB 粒度）");
        assert!(d, "D=1（32 位）");
        assert!(!l, "L=0（不是 64 位）");
        assert_eq!(byte_of(build_gdt()[3], 5), 0b1001_1011, "代码段 access");
    }

    #[test]
    fn selector_0x28_is_a_64_bit_code_segment() {
        let (_, d, l) = flags(build_gdt()[5]);
        assert!(l, "L=1（64 位）");
        assert!(!d, "D 必须为 0（64 位代码段）");
        assert_eq!(byte_of(build_gdt()[5], 5), 0b1001_1011, "代码段 access");
    }

    #[test]
    fn selector_0x20_is_a_32_bit_data_segment() {
        let (g, d, l) = flags(build_gdt()[4]);
        assert!(g && d && !l, "32 位数据段");
        assert_eq!(byte_of(build_gdt()[4], 5), 0b1001_0011, "数据段 access");
    }

    #[test]
    fn selector_0x30_is_a_64_bit_data_segment() {
        assert_eq!(byte_of(build_gdt()[6], 5), 0b1001_0011, "数据段 access");
    }

    #[test]
    fn selector_0x38_is_a_tss() {
        assert_eq!(byte_of(build_gdt()[7], 5), 0x89, "TSS access（available）");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::{layout, ARG_FRAME_BYTES, LOW_LIMIT, LOW_STACK_BYTES};

    #[test]
    fn arg_frame_is_thirteen_dwords() {
        assert_eq!(ARG_FRAME_BYTES, 13 * 4, "11 个参数 + mode64_low + hh_low");
    }

    #[test]
    fn segments_are_aligned_ordered_and_the_frame_is_last() {
        let (go32, sp32, stack, args, total) = layout(0x137, 0x2A1);
        assert_eq!(go32, 0, "go32 在最前");
        assert_eq!(sp32 % 16, 0, "spinup32 必须 16 字节对齐");
        assert!(sp32 >= 0x137, "spinup32 不覆盖 go32");
        assert_eq!(stack % 16, 0, "栈区必须 16 字节对齐");
        assert!(stack >= sp32 + 0x2A1, "栈区不覆盖 spinup32");
        assert_eq!(args % 16, 0, "参数帧必须 16 字节对齐");
        assert_eq!(args, stack + LOW_STACK_BYTES, "参数帧紧跟栈区，栈向下生长");
        assert_eq!(total, args + ARG_FRAME_BYTES, "total 以参数帧结尾");
    }

    #[test]
    fn total_covers_everything_even_for_zero_lengths() {
        let (go32, sp32, stack, args, total) = layout(0, 0);
        assert_eq!((go32, sp32, stack), (0, 0, 0));
        assert_eq!(args, LOW_STACK_BYTES);
        assert_eq!(total, LOW_STACK_BYTES + ARG_FRAME_BYTES);
    }

    #[test]
    fn low_limit_is_four_gib() {
        assert_eq!(LOW_LIMIT, 4 * 1024 * 1024 * 1024);
    }
}

#[cfg(test)]
mod args_tests {
    use super::SpinupArgs;
    use core::mem::size_of;

    #[test]
    fn arg_struct_is_eleven_dwords() {
        assert_eq!(size_of::<SpinupArgs>(), 44, "11 个 dword = 44 字节");
    }

    #[test]
    fn entry_stack_and_dmo_are_adjacent_lo_hi_pairs() {
        // 汇编按 qword 读 [rsp+8]/[rsp+16]/[rsp+32]，因此 lo/hi 必须相邻且 lo 在前。
        let args = SpinupArgs {
            level5pg: 0,
            pagemap_top: 0,
            entry_lo: 0x1111_1111,
            entry_hi: 0x2222_2222,
            stack_lo: 0x3333_3333,
            stack_hi: 0x4444_4444,
            gdt: 0,
            nx_available: 0,
            dmo_lo: 0x5555_5555,
            dmo_hi: 0x6666_6666,
            base_revision: 0,
        };
        let raw = &args as *const SpinupArgs as *const u32;
        // SAFETY: 结构体是 repr(C)、11 个 u32；按 11 个 u32 读是合法的。
        let words: [u32; 11] = unsafe { core::ptr::read(raw as *const [u32; 11]) };
        assert_eq!(words[2], args.entry_lo);
        assert_eq!(words[3], args.entry_hi);
        assert_eq!(words[4], args.stack_lo);
        assert_eq!(words[5], args.stack_hi);
        assert_eq!(words[8], args.dmo_lo);
        assert_eq!(words[9], args.dmo_hi);
    }
}
