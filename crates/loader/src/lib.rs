//! 装载层：内核映像的解析与装载（L4）。
//!
//! 边界：本 crate 只依赖抽象（`arch`、`firmware`）；文件内容由 `fs` 提供，
//! 本 crate 不直接依赖 `efi` 或 `x86_64`（ADR-050）。

#![no_std]

#[cfg(test)]
extern crate std;
