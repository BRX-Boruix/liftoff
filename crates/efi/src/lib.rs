//! UEFI 类型与协议绑定：唯一直接接触固件的 crate。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod types;

pub mod boot_services;

pub mod block_io;

pub mod file;

pub mod graphics;

pub mod boot_services_table;

pub mod memory;

pub mod memory_map;

pub mod status;

pub mod memory_map_source;

pub mod uefi_memory_source;

pub mod uefi_boot_services;

pub mod handles;

pub mod block_read;

pub mod block_source;

pub mod enumerate;

pub mod guid;

pub mod protocol;

pub mod protocol_lookup;

pub mod discover;
