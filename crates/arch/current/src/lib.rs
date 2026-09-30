//! 架构实现选择器：把具体实现接到抽象上（ADR-050）。

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(all(feature = "impl-x86_64", feature = "impl-mock"))]
compile_error!("`impl-x86_64` 与 `impl-mock` 互斥，只能启用一个");

#[cfg(not(any(feature = "impl-x86_64", feature = "impl-mock")))]
compile_error!("必须启用一个实现 feature：`impl-x86_64` 或 `impl-mock`");

#[cfg(feature = "impl-x86_64")]
pub use x86_64 as current;

#[cfg(feature = "impl-mock")]
pub mod current {
    //! 宿主测试用的最小实现（随抽象层逐步充实）。
}
