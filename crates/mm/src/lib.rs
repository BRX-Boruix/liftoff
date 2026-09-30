//! 内存层：物理页来源与分页构建（L3）。
//!
//! 边界：本 crate **只依赖抽象**（`arch` 与 `firmware`），不直接依赖任何实现
//! （`x86_64`/`efi`）；实现的选择与装配由选择器与入口负责（ADR-050）。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod apply;
pub mod frame_allocator;
pub mod plan;
pub mod takeover;
pub mod usable;
