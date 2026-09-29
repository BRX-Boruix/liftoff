//! M8 模块协议验收消费者（测试内核）。
//!
//! 只做三件事：声明 `BaseRevision` 与 `ModuleRequest` 两个 Limine 标记、
//! 打印 bootloader 填好的模块清单、停机。它是"liftoff 真的把模块交出去了"
//! 的独立消费者——BORUIX 内核不声明模块请求，故其 ADR-028 单源架构不受影响。
#![no_std]
#![no_main]

use core::panic::PanicInfo;

// ---------------------------------------------------------------- 串口（COM1，EBS 后自初始化）

const COM1: u16 = 0x3F8;
const LSR_THR_EMPTY: u8 = 0x20;

fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value,
            options(nomem, nostack, preserves_flags));
    }
}

fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") v, in("dx") port,
            options(nomem, nostack, preserves_flags));
    }
    v
}

/// 115200 8N1（与 liftoff serial::init 同一寄存器序列）。
fn serial_init() {
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x80);
    outb(COM1 + 0, 0x01);
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03);
    outb(COM1 + 2, 0xC7);
    outb(COM1 + 4, 0x0B);
}

fn putc(b: u8) {
    while inb(COM1 + 5) & LSR_THR_EMPTY == 0 {
        core::hint::spin_loop();
    }
    outb(COM1 + 0, b);
}

fn puts(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' { putc(b'\r'); }
        putc(b);
    }
}

fn put_dec(mut v: u64) {
    if v == 0 { putc(b'0'); return; }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] { putc(b); }
}

fn put_hex16(v: u16) {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut shift = 12i32;
    while shift >= 0 {
        putc(H[((v >> shift) & 0xF) as usize]);
        shift -= 4;
    }
}

// ---------------------------------------------------------------- 受限内存访问

fn r8(a: u64) -> u8 { unsafe { core::ptr::read_volatile(a as *const u8) } }
fn r32(a: u64) -> u32 { unsafe { core::ptr::read_volatile(a as *const u32) } }
fn r64(a: u64) -> u64 { unsafe { core::ptr::read_volatile(a as *const u64) } }

fn sum16(addr: u64, len: u64) -> u16 {
    let mut s: u32 = 0;
    let mut i = 0u64;
    while i < len {
        s = s.wrapping_add(r8(addr + i) as u32);
        i += 1;
    }
    s as u16
}

/// 打印 NUL 结尾字符串（上限 128 字节，防御坏指针）。
fn cstr(addr: u64) {
    if addr == 0 { puts("<null>"); return; }
    let mut i = 0u64;
    while i < 128 {
        let b = r8(addr + i);
        if b == 0 { return; }
        putc(b);
        i += 1;
    }
}

// ---------------------------------------------------------------- Limine 标记

const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];
const MODULE_ID: [u64; 2] = [0x3e7e279702be32af, 0xca1c4f3bd1280cee];
const BASE_REVISION_ID: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

/// ModuleRequest：COMMON_MAGIC + id + revision + response。
/// 「请求标记 48B」布局（brxlimine-rs make_struct!）：response 在 +40。
#[used]
#[link_section = ".limine_reqs"]
static MODULE_REQUEST: [u64; 6] = [
    COMMON_MAGIC[0], COMMON_MAGIC[1], MODULE_ID[0], MODULE_ID[1], 0, 0,
];

/// BaseRevision：id(2) + revision（liftoff 覆写为支持的最高版本）。
#[used]
#[link_section = ".limine_reqs"]
static BASE_REVISION: [u64; 3] = [BASE_REVISION_ID[0], BASE_REVISION_ID[1], 6];

// ---------------------------------------------------------------- 入口

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    serial_init();
    puts("[modtest] alive\n");

    let req = core::ptr::addr_of!(MODULE_REQUEST) as u64;
    let breq = core::ptr::addr_of!(BASE_REVISION) as u64;
    let base_rev = r64(breq + 16);
    puts("[modtest] baserev=");
    put_dec(base_rev);
    putc(b'\n');

    let resp = r64(req + 40);
    if resp == 0 {
        puts("[modtest] no module response\n");
        halt();
    }
    let count = r64(resp + 8);
    let ptrs = r64(resp + 16);
    puts("[modtest] count=");
    put_dec(count);
    putc(b'\n');

    let mut i = 0u64;
    while i < count && i < 16 {
        let fp = r64(ptrs + i * 8);
        let base = r64(fp + 8);
        let len = r64(fp + 16);
        let path = r64(fp + 24);
        let cmd = r64(fp + 32);
        let media = r32(fp + 40);
        puts("[modtest] m");
        put_dec(i);
        puts(" path=");
        cstr(path);
        puts(" len=");
        put_dec(len);
        puts(" media=");
        put_dec(media as u64);
        puts(" sum16=0x");
        put_hex16(sum16(base, len));
        puts(" cmd=");
        cstr(cmd);
        putc(b'\n');
        i += 1;
    }
    puts("[modtest] done\n");
    halt();
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    puts("[modtest] panic\n");
    halt();
}
