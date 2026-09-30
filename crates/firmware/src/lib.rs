//! 固件层抽象：引导器与固件（UEFI / BIOS）之间的唯一接口面（ADR-049 EXT-1）。
//!
//! 本 crate 只放**抽象**（类型与 trait）；实现由 `crates/efi`（UEFI）与未来的
//! `crates/bios` 提供。抽象不出现任何固件专有名词。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod error;
pub mod memory;