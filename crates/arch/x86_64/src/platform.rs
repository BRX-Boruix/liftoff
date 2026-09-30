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
        // SAFETY: 由调用方保证（见 trait 的 SAFETY 契约）。
        unsafe {
            core::arch::asm!("jmp {entry}", entry = in(reg) entry, options(noreturn));
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
    use super::X86_64;
    use arch::platform::Platform;

    #[test]
    fn reports_the_architecture_name() {
        assert_eq!(<X86_64 as Platform>::name(), "x86_64");
    }
}