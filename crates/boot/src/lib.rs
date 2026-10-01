//! liftoff 引导逻辑（可宿主测试的部分）。
//!
//! 边界：本 crate 的 **bin** 只做最薄的入口（取参数、调用这里的函数）；
//! 真正的逻辑放在本 lib 里，从而能在宿主上用 `cargo test -p boot` 测透。

#![no_std]

#[cfg(test)]
extern crate std;

/// 当前选定的平台实现（由 `current` 选择器决定）。
///
/// 入口（bin）与本 lib 都经它使用平台能力；**实现的选择集中在这里**，入口不自己挑实现。
pub use current::PlatformImpl;

#[cfg(test)]
pub mod test_support;

pub mod diag;
pub mod entry;
pub mod media;
pub mod protocol;
pub mod responses;