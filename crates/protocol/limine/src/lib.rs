//! Limine 协议层：请求扫描与响应填充。
//!
//! 边界：本 crate 只负责协议本身（请求/响应结构与填充规则），不含引导流程、分页或固件访问。
//! 下文每个结构与常量都对照 brxLimine/limine-protocol/include/limine.h 核实，不凭记忆书写。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod base;
pub mod framebuffer;
pub mod memmap;
pub mod mp;
pub mod rsdp;
