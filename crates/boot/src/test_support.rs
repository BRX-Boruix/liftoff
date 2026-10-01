//! 宿主测试共用的辅助：把「真实产物」的读取**单点定义**（S15）。
//!
//! 为什么要有它：`entry.rs` 与 `responses.rs` 的测试都需要读真实 ISO 里的内核
//! 映像。各写一份读取逻辑，就会出现「一处改了、另一处还在读旧偏移」的经典分叉 ——
//! 而这类分叉的症状是「测试仍然通过，但测的不是同一个东西」。
//!
//! 只在测试构建下存在（`#[cfg(test)]`）：产物读取不属于引导器运行时代码。

/// 真实内核映像在 ISO 里的路径分量。与 `tools_build/config.py` 的
/// `KERNEL_ISO_COMPONENTS` 是同一个事实的两种表达 —— 诊断工具按路径查，
/// 这里按已验证的 LBA/长度读（宿主测试不必实现 ISO9660 解析）。
const KERNEL_LBA: u64 = 33;
const BLOCK_SIZE: u64 = 2048;
const KERNEL_BYTES: usize = 24_619_400;

/// 读真实内核映像；ISO 不存在时返回 `None`（调用方应跳过而非假装通过）。
///
/// 注意：返回 `None` 的测试**必须**打印跳过原因。静默跳过会让「产物不在」
/// 伪装成「测试通过」——本项目在真实产物测试上踩过这个坑。
pub fn real_kernel() -> Option<std::vec::Vec<u8>> {
    let iso = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../boruix.iso");
    let bytes = std::fs::read(iso).ok()?;
    let start = usize::try_from(KERNEL_LBA * BLOCK_SIZE).ok()?;
    bytes.get(start..start + KERNEL_BYTES).map(|s| s.to_vec())
}