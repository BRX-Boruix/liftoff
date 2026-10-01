//! spinup trampoline（L5）。
//!
//! **为什么需要**：UEFI 环境里直接 jmp 进内核会死 —— 固件留下的分页模式、
//! 段描述符、GDT/IDT、EFER 都是固件的，而内核假设的是 Limine 协议定义的干净状态
//! （真机实测：Exit 成功、激活成功、跳转后即复位循环）。旧实现（brxLimine）的
//! 路径是：把 32 位重置代码**复制到低地址**（Exit 后 64 位 EIP 不可靠），降级到
//! 32 位（关分页）、按协议重设全部机器状态、再重进 64 位并 iretq 进内核。
//!
//! 参考实现：brxLimine common/lib/spinup.asm_uefi_x86_64、
//! common/protos/limine_32.asm_x86、common/lib/misc.c:37-105、sys/gdt.s2.c。

/// 组装 GDT：与 brxLimine gdt.s2.c 的 8 个描述符逐字段一致（小端 8 字节）。
/// 布局：0 空、1 32 位代码(0x18)、2 32 位数据(0x20)、3 64 位代码(0x28)、
/// 4 64 位数据(0x30)、5 64 位代码(gran=1)、6 64 位数据、7 TSS 低、8 TSS 高。
pub fn build_gdt() -> [u64; 9] {
    // gran 是旧实现里的「granularity」字段：**完整的第 6 字节**（G/D/L 标志 +
    // limit 高 4 位）—— 不要自己拆 limit，直接照抄旧实现的字节值。
    let desc = |limit: u32, base: u32, access: u8, gran: u8| -> u64 {
        let limit_low = (limit & 0xFFFF) as u64;
        let limit_hi = ((limit >> 16) & 0xF) as u64;
        let base_low = (base & 0xFFFF) as u64;
        let base_mid = ((base >> 16) & 0xFF) as u64;
        let base_hi = ((base >> 24) & 0xFF) as u64;
        limit_low
            | (base_low << 16)
            | (base_mid << 32)
            | ((access as u64) << 40)
            | (limit_hi << 48)
            | ((gran as u64) << 52)
            | (base_hi << 56)
    };
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

#[cfg(test)]
mod tests {
    use super::build_gdt;

    #[test]
    fn gdt_entries_match_the_reference_layout() {
        let gdt = build_gdt();
        assert_eq!(gdt[0], 0, "描述符 0 必须为空");
        // 32 位代码 (0x18)：access=10011011b=0x9B, gran 字节=0
        assert_eq!(gdt[1] >> 40 & 0xFF, 0x9B);
        // 32 位数据 (0x20)：access=10010011b=0x93
        assert_eq!(gdt[2] >> 40 & 0xFF, 0x93);
        // 64 位代码 (0x28)：access=0x9B，gran 字节 = 0b1100_1111（G=1、D=0、L=1、limit_hi=1111）
        assert_eq!(gdt[3] >> 40 & 0xFF, 0x9B);
        assert_eq!(gdt[3] >> 52 & 0xFF, 0b1100_1111);
        // 64 位数据 (0x30)：access=0x93
        assert_eq!(gdt[4] >> 40 & 0xFF, 0x93);
        // TSS（0x38）：access=0x89
        assert_eq!(gdt[7] >> 40 & 0xFF, 0x89);
    }
}

// 三段式 trampoline 的汇编体。**复制到低地址执行**（Exit 后 64 位 EIP 不可靠）。
// 参数布局（32 位压栈，[esp] 起，对照 common_spinup 的 push32 序）：
//   [esp+0]  level5pg        [esp+4]  pagemap_top_lo   [esp+8]  pagemap_top_hi
//   [esp+12] entry_lo        [esp+16] entry_hi        [esp+20] stack_lo
//   [esp+24] stack_hi        [esp+28] gdt_lo          [esp+32] gdt_hi
//   [esp+36] nx_available    [esp+40] dmo_lo          [esp+44] dmo_hi
//   [esp+48] unmap_lower
#[cfg(target_os = "uefi")]
core::arch::global_asm!(
    ".section .text.spinup",
    ".global spinup_go32",
    ".global spinup_go32_end",
    ".global limine_spinup_32",
    ".global limine_spinup_32_end",
    ".global spinup_gdt_ptr",
    ".global spinup_idt_ptr",
    ".global spinup_text_start",
    ".global spinup_text_end",
    "spinup_text_start:",
    "spinup_common64:",
    // 64 位入口（Exit 后从 enter_kernel 跳到这里）：加载自建 GDT/IDT，降 32 位。
    // rdi = spinup_go32 低地址拷贝，rsi = 低地址栈顶，rdx = 32 位参数区指针。
    "    cli",
    "    lgdt [rip + spinup_gdt_ptr]",
    "    lidt [rip + spinup_idt_ptr]",
    "    lea rbx, [rip + .reload_cs]",
    "    push 0x28",
    "    push rbx",
    "    retfq",
    ".reload_cs:",
    "    mov eax, 0x30",
    "    mov ds, eax",
    "    mov es, eax",
    "    mov fs, eax",
    "    mov gs, eax",
    "    mov ss, eax",
    "    mov rsp, rsi",
    "    push 0x18",
    "    call rdx",
    // 32 位段：关分页、清 CR0/CR3/CR4/EFER（对照 spinup_go32）。
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
    "    ret",
    "spinup_go32_end:",
    // 32 位重升：PAT、LA57、CR0.WP、CR3=新表、PAE、EFER、CR0.PG、重进 64。
    "limine_spinup_32:",
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
    "    cmp dword ptr [esp+0], 0",
    "    je 3f",
    "    mov eax, cr4",
    "    bts eax, 12",
    "    mov cr4, eax",
    "3:",
    "    mov eax, cr0",
    "    bts eax, 16",
    "    mov cr0, eax",
    "    cld",
    "    mov eax, [esp+4]",
    "    mov cr3, eax",
    "    mov eax, cr4",
    "    bts eax, 5",
    "    mov cr4, eax",
    "    mov ecx, 0xc0000080",
    "    xor edx, edx",
    "    mov eax, 1 << 8",
    "    cmp dword ptr [esp+28], 0",
    "    je 4f",
    "    or eax, 1 << 11",
    "4:",
    "    wrmsr",
    "    mov eax, cr0",
    "    bts eax, 31",
    "    mov cr0, eax",
    "    push 0x28",
    "    call 5f",
    "5:",
    "    mov ebx, 6f",
    "    sub ebx, 5b",
    "    add dword ptr [esp], ebx",
    "    retf",
    ".code64",
    "6:",
    "    mov eax, 0x30",
    "    mov ds, eax",
    "    mov es, eax",
    "    mov fs, eax",
    "    mov gs, eax",
    "    mov ss, eax",
    "    mov rax, [rsp+28]",
    "    lgdt [rax]",
    "    mov rax, [rsp+36]",
    "    add rsp, rax",
    "    call 7f",
    "7:",
    "    mov r10, 8f",
    "    sub r10, 7b",
    "    add qword ptr [rsp], r10",
    "    add qword ptr [rsp], rax",
    "    retf",
    "8:",
    "    cmp dword ptr [rsp+44], 1",
    "    jb 9f",
    "    mov rsi, cr3",
    "    lea rdi, [rsi + rax]",
    "    mov rcx, 256",
    "    xor rax, rax",
    "    rep stosq",
    "    mov cr3, rsi",
    "9:",
    "    mov rsi, [rsp+20]",
    "    sub rsi, 8",
    "    mov qword ptr [rsi], 0",
    "    mov rax, [rsp+12]",
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
    "limine_spinup_32_end:",
    "spinup_text_end:",
    // 自建 GDT（build_gdt() 的字节在运行时填入 spinup_gdt）+ GDTR/IDTR。
    ".align 8",
    "spinup_gdt:",
    "    .quad 0, 0, 0, 0, 0, 0, 0, 0, 0",
    "spinup_gdt_ptr:",
    "    .word 71",
    "    .quad spinup_gdt",
    "spinup_idt_ptr:",
    "    .word 0",
    "    .quad 0",
);

// Exit 后传给 32 位 trampoline 的参数区布局（对照 common_spinup 的 11 个压栈参数）。
// 由 Rust 在 Exit 前填充进低地址缓冲的参数区（32 位压栈序：低地址在前 ✗ —— 实际
// 是压栈序，即高地址是第一个参数 ✗；这里按 [esp+i*4] 索引：esp 指向最后一个压栈的）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpinupArgs {
    /// 5 级分页？（我们不启用）。
    pub level5pg: u32,
    /// PML4 物理地址（低 4G 内，高 32 位为 0）。
    pub pagemap_top: u32,
    /// 内核入口低 32 位。
    pub entry_lo: u32,
    /// 内核入口高 32 位。
    pub entry_hi: u32,
    /// 内核栈顶低 32 位。
    pub stack_lo: u32,
    /// 内核栈顶高 32 位。
    pub stack_hi: u32,
    /// 自建 GDT 指针（低 4G 内）。
    pub gdt: u32,
    /// NX 可用（1）。
    pub nx_available: u32,
    /// direct map offset 低 32 位。
    pub dmo_lo: u32,
    /// direct map offset 高 32 位。
    pub dmo_hi: u32,
    /// base revision（1 = unmap lower half）。
    pub base_revision: u32,
}

#[cfg(target_os = "uefi")]
unsafe extern "C" {
    static spinup_text_start: u8;
    static spinup_text_end: u8;
    static spinup_go32: u8;
    #[allow(unused)]
    fn spinup_common64();
}

/// Exit 前调用：把汇编体、GDT、参数、低栈搬进 `buffer`（< 4 GiB）。
/// 返回 `(go32_entry, stack_top, args_ptr)` —— 全部是低地址。
///
/// # Safety
///
/// `buffer` 必须指向 `buffer_len` 的可写内存，且 Exit 后保持有效。
#[cfg(target_os = "uefi")]
pub unsafe fn stage_low_buffer(
    buffer: *mut u8,
    buffer_len: usize,
    args: &SpinupArgs,
) -> Option<(usize, usize, usize)> {
    let text_start = &raw const spinup_text_start as usize;
    let go32_sym = &raw const spinup_go32 as usize;
    let text_end = &raw const spinup_text_end as usize;
    let text_len = text_end.checked_sub(text_start)?;
    let total = text_len + 72 + 52 + 4096;
    if buffer_len < total {
        return None;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(text_start as *const u8, buffer, text_len);
        let gdt_at = buffer as usize + text_len;
        for (index, word) in build_gdt().iter().enumerate() {
            core::ptr::write_unaligned((gdt_at + index * 8) as *mut u64, *word);
        }
        let args_at = gdt_at + 72;
        let go32_low = buffer as usize + ((go32_sym) - text_start);
        let _ = go32_low;
        let words = [
            args.level5pg, args.pagemap_top, args.entry_lo, args.entry_hi,
            args.stack_lo, args.stack_hi, args.gdt, args.nx_available,
            args.dmo_lo, args.dmo_hi, args.base_revision, 0, 0,
        ];
        for (index, word) in words.iter().enumerate() {
            core::ptr::write_unaligned((args_at + index * 4) as *mut u32, *word);
        }
    }
    let stack_top = buffer as usize + total;
    let go32_low = buffer as usize + ((&raw const spinup_go32 as usize) - text_start);
    Some((go32_low, stack_top, buffer as usize + text_len + 72))
}

#[cfg(not(target_os = "uefi"))]
pub unsafe fn stage_low_buffer(
    _buffer: *mut u8,
    _buffer_len: usize,
    _args: &SpinupArgs,
) -> Option<(usize, usize, usize)> {
    None // 宿主无 trampoline；等价性由真机验证
}

/// Exit 后调用：跳进低地址 trampoline（不返回）。
///
/// # Safety
///
/// 只能 Exit 成功后调用一次；三个指针必须来自 [`stage_low_buffer`]。
#[cfg(target_os = "uefi")]
pub unsafe fn spinup_go(go32: usize, stack_top: usize, args: usize, enter_addr: usize) -> ! {
    // SAFETY: 调用方保证三指针来自 stage_low_buffer 且 Exit 已成功；
    // jmp 目标是本模块汇编导出的 64 位入口（不返回）。
    unsafe {
        core::arch::asm!(
            "mov rdi, {go32}",
            "mov rsi, {stack}",
            "mov rdx, {args}",
            "jmp {enter}",
            go32 = in(reg) go32,
            stack = in(reg) stack_top,
            args = in(reg) args,
            enter = in(reg) enter_addr,
            options(noreturn),
        )
    }
}

#[cfg(not(target_os = "uefi"))]
pub unsafe fn spinup_go(_go32: usize, _stack_top: usize, _args: usize, _enter: usize) -> ! {
    // 宿主测试二进制无法链接裸 32 位 trampoline —— 该调用只发生在真机。
    unreachable!("spinup_go 只在 UEFI 目标上有意义")
}