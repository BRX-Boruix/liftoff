//! Limine protocol layer: request scanning and response filling.
//!
//! Boundary: this crate owns the protocol itself (request/response structures and
//! filling rules) only. It contains no boot flow, paging or firmware access.
//! The protocol reference is brxLimine/limine-protocol/include/limine.h.

#![no_std]

#[cfg(test)]
extern crate std;
