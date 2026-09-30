//! x86_64 架构实现：CPU、GDT/IDT、LAPIC、分页、SMP 与 AP trampoline。

#![no_std]

#[cfg(test)]
extern crate std;
