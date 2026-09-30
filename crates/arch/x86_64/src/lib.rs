//! x86_64 架构实现。
//!
//! 汇编边界（ADR-051）：单条语义操作使用内联 `asm!`；需要整块复制到低内存执行的
//! trampoline 另置于 `global_asm!`（尚未实现）。

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(test)]
extern crate std;

pub mod platform;
