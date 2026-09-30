//! Limine protocol layer: request scanning and response filling.
//!
//! Boundary: this crate owns the protocol itself (request/response structures and
//! filling rules) only. It contains no boot flow, paging or firmware access.
//! Every structure below is verified against
//! brxLimine/limine-protocol/include/limine.h (not written from memory).

#![no_std]

#[cfg(test)]
extern crate std;

pub mod base;
pub mod memmap;
