//! COM1（16550 UART）直写通道。
//!
//! 与固件无关的输出通道：固件控制台不可用时串口仍然可用。
//! QEMU 下经 `-serial stdio` 直达宿主标准输出，是本工程的观测手段。

use core::arch::asm;
use core::fmt;

/// COM1 基址。
const COM1: u16 = 0x3F8;

// 端口偏移。DLAB=1 时偏移 0/1 变为波特率除数的低/高字节。
const REG_DATA: u16 = 0; // THR / RBR / DLL
const REG_IER: u16 = 1; // IER / DLM
const REG_FCR: u16 = 2;
const REG_LCR: u16 = 3;
const REG_MCR: u16 = 4;
const REG_LSR: u16 = 5;

/// 线路状态寄存器：发送保持寄存器空。
const LSR_THR_EMPTY: u8 = 0x20;

const LCR_8N1: u8 = 0x03;
const LCR_DLAB: u8 = 0x80;
const FCR_ENABLE_FIFO: u8 = 0xC7;
const MCR_DTR_RTS_OUT2: u8 = 0x0B;

/// 波特率除数：115200 / 115200 = 1。
const DIVISOR_115200: u16 = 1;

fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

/// 按 115200 8N1 初始化 COM1。可重复调用（panic 路径也会调）。
pub fn init() {
    outb(COM1 + REG_IER, 0x00); // 关中断
    outb(COM1 + REG_LCR, LCR_DLAB);
    outb(COM1 + REG_DATA, (DIVISOR_115200 & 0xFF) as u8);
    outb(COM1 + REG_IER, (DIVISOR_115200 >> 8) as u8);
    outb(COM1 + REG_LCR, LCR_8N1);
    outb(COM1 + REG_FCR, FCR_ENABLE_FIFO);
    outb(COM1 + REG_MCR, MCR_DTR_RTS_OUT2);
}

/// 阻塞发送一个字节。
pub fn send_byte(byte: u8) {
    while inb(COM1 + REG_LSR) & LSR_THR_EMPTY == 0 {
        core::hint::spin_loop();
    }
    outb(COM1 + REG_DATA, byte);
}

/// 串口写入器，供 `write!` 使用。
pub struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if b == b'\n' {
                send_byte(b'\r');
            }
            send_byte(b);
        }
        Ok(())
    }
}

/// 向串口写一段格式化文本。格式化失败被忽略：观测通道不得反噬主流程。
pub fn write(args: fmt::Arguments) {
    let mut w = Writer;
    let _ = fmt::write(&mut w, args);
}

#[macro_export]
macro_rules! sprint {
    ($($arg:tt)*) => {
        $crate::serial::write(format_args!($($arg)*))
    };
}
