//! 驱动层：块设备等硬件抽象的接入（L4）。
//!
//! 边界：本 crate 只依赖抽象（`arch`、`firmware`）；具体实现由 `firmware-current` 门面注入，
//! 本 crate 不直接依赖 `efi` 或 `x86_64`（ADR-050）。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod crc32;
pub mod gpt;
pub mod partition;
pub mod table;
