//! UEFI 类型与协议绑定：唯一直接接触固件的 crate。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod types;

pub mod boot_services;

pub mod block_io;

pub mod file;

pub mod graphics;
