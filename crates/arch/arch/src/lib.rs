//! 架构抽象：平台无关的 trait 与类型（见 ADR-007、ADR-050）。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod addr;
pub mod paging;
pub mod hhdm;
