//! 构建期与运行期配置常量。
//!
//! 所有路径与协议版本集中在此，避免散落的字面量。

/// 内核在引导介质上的绝对路径。
///
/// 两种启动模式（liveCD 的 ISO9660 / 安装模式的 EXT2）路径一致。
pub const KERNEL_PATH: &str = "/boot/kernel";

/// M2a 验收用测试文件（引导介质根目录）。
/// 验收脚本 boottest.ps1 以同名常量生成内容并预计算校验和。
pub const TEST_FILE: &str = "m2a.txt";

/// M2b 阶段 ISO 内的内核路径（fixture 布局：/KERNEL/KERNIMG.BIN）。
/// 未来 xorriso 生成的真实 liveCD 沿用此布局与大小写不敏感匹配。
pub const ISO_KERNEL_PATH: &str = "KERNEL/KERNIMG.BIN";

/// M2c 阶段 EXT2 内的内核路径（fixture 布局：/BOOT/KERNIMG.BIN）。
/// 未来 mke2fs 生成的安装镜像沿用此布局。
pub const EXT_KERNEL_PATH: &str = "BOOT/KERNIMG.BIN";

// ----------------------------------------------------------------
// 引导协议（阶段一：复用 Limine 协议）
//
// 协议分两阶段实施，规划见 wiki/contributor/liftoff.md。
// 本模块的常量对应阶段一，与内核 `brxlimine-rs` 的口径必须一致。
// ----------------------------------------------------------------

/// 支持的 Limine 协议基础版本号。
///
/// 与内核 `BaseRevision::new(6)` 保持一致。
/// 阶段二自拟协议将取消版本协商，改为 magic + version 精确匹配。
pub const LIMINE_BASE_REVISION: u64 = 6;

// Limine 协议 `File.media_type` 取值（GENERIC=0 普通磁盘 / OPTICAL=1 光盘）
// 属 M4 契约，在 contributor 文档与本注释记录；M4 引入 File 响应结构时
// 随使用回归本模块。