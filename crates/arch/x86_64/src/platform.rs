//! `arch::Platform` 的 x86_64 实现。

use arch::platform::{InterruptState, Platform};

/// x86_64 平台。
pub struct X86_64;

/// COM1 数据寄存器（其余寄存器按 16550 约定偏移）。
const COM1: u16 = 0x3F8;

/// 发送保持寄存器空的等待上界。
///
/// 取值理由：16550 在 115200 baud 下每字节约 87 µs；10 万次端口读远超该量级，
/// 既能容忍慢速 UART，又不会在 UART 缺失时死等（严格模式：失败模式优先）。
const TX_WAIT_LIMIT: u32 = 100_000;

impl Platform for X86_64 {
    fn name() -> &'static str {
        "x86_64"
    }

    fn init() {
        // 16550 初始化：115200 8N1、FIFO 使能并清空、DTR/RTS 置位；
        // 引导阶段只走轮询，故关闭中断。
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1 + 0, 0x01);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x03);
    }

    unsafe fn jump_to(entry: u64) -> ! {
        // Limine 协议只保证 RSP 必须指向有效栈：固件栈在 ExitBootServices 之后的
        // 状态未知，所以这里改用调用方预留的参数（RDI = 入口前栈顶）作为新栈，
        // 再跳入内核。RDI 同时保留入口参数语义由调用方约定。
        // SAFETY: 由调用方保证（见 trait 的 SAFETY 契约）。
        unsafe {
            core::arch::asm!(
                "mov rsp, {new_stack}",
                "xor rbp, rbp",
                "jmp {entry}",
                new_stack = in(reg) 0xffff_ff80_80ae_d000u64,
                entry = in(reg) entry,
                options(noreturn),
            );
        }
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `hlt` 在 ring 0 合法；不访问内存、不改变通用寄存器。
            unsafe {
                core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
            }
        }
    }

    fn write_byte(byte: u8) {
        for _ in 0..TX_WAIT_LIMIT {
            if inb(COM1 + 5) & 0x20 != 0 {
                break;
            }
        }
        outb(COM1, byte);
    }

    fn bsp_lapic_id() -> u32 {
        // CPUID.1:EBX[31:24] 是本地 APIC ID（xAPIC 布局；x2APIC 下这 8 位仍有效）。
        // `__cpuid` 非特权，宿主也能执行 —— 但**宿主结果无意义**，所以逻辑抽成纯函数
        // 单独测（见 `lapic_id_from_cpuid1_ebx`），不把测试绑到这台机器上。
        lapic_id_from_cpuid1_ebx(core::arch::x86_64::__cpuid(1).ebx)
    }

    fn disable_interrupts() -> InterruptState {
        let flags: u64;
        // SAFETY: 读取 RFLAGS 与关中断在 ring 0 合法；两者都不访问内存。
        unsafe {
            core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(nomem, nostack, preserves_flags));
            core::arch::asm!("cli", options(nomem, nostack));
        }
        InterruptState::from_enabled(flags & (1 << 9) != 0)
    }

    fn restore_interrupts(state: InterruptState) {
        if state.was_enabled() {
            // SAFETY: 仅在调用前中断处于开启状态时恢复开启；`sti` 不改内存。
            unsafe {
                core::arch::asm!("sti", options(nomem, nostack));
            }
        }
    }
}

/// 从 `CPUID.1:EBX` 取出本地 APIC ID（高 8 位）。
///
/// 抽成纯函数是为了可测：`CPUID` 的结果取决于当前机器，直接断言 `bsp_lapic_id()`
/// 会把测试绑到硬件上；这里只断言**取位逻辑**。
pub const fn lapic_id_from_cpuid1_ebx(ebx: u32) -> u32 {
    ebx >> 24
}

/// 读一个字节端口。
#[inline]
fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: 端口读在 ring 0 合法；`dx`/`al` 由指令显式指定。
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// 写一个字节端口。
#[inline]
fn outb(port: u16, value: u8) {
    // SAFETY: 端口写在 ring 0 合法；`dx`/`al` 由指令显式指定。
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

#[cfg(test)]
mod tests {
    use super::lapic_id_from_cpuid1_ebx;

    #[test]
    fn lapic_id_comes_from_the_high_byte_of_ebx() {
        // 只断言**取位逻辑**：CPUID 的真实结果取决于当前机器，把它写进断言就是
        // 把测试绑到硬件上。
        assert_eq!(lapic_id_from_cpuid1_ebx(0x0000_0000), 0);
        assert_eq!(lapic_id_from_cpuid1_ebx(0xAB00_0000), 0xAB);
        assert_eq!(lapic_id_from_cpuid1_ebx(0xFF00_0000), 0xFF);
        // 低 24 位不得影响结果。
        assert_eq!(lapic_id_from_cpuid1_ebx(0x1200_FFFF), 0x12);
    }
    use super::X86_64;
    use arch::platform::Platform;

    #[test]
    fn reports_the_architecture_name() {
        assert_eq!(<X86_64 as Platform>::name(), "x86_64");
    }
}