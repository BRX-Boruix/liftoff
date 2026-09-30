//! 文件系统层：文件抽象与各文件系统实现（L4）。
//!
//! 边界：本 crate 只依赖抽象（`arch`、`firmware`）；介质读取经固件抽象，
//! 本 crate 不直接依赖 `efi` 或 `x86_64`（ADR-050）。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod ext2;
pub mod fat;
pub mod iso9660;
