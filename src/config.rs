//! 构建期与运行期配置常量。
//!
//! 所有路径与协议版本集中在此，避免散落的字面量。

/// UEFI 成功状态码。
pub const EFI_SUCCESS: usize = 0;

/// 内核在引导分区上的绝对路径。
pub const KERNEL_PATH: &str = "/boot/kernel";

/// liftoff.conf 路径。M2 起用于覆盖 KERNEL_PATH。
pub const CONFIG_PATH: &str = "/boot/liftoff.conf";

/// 支持的 Limine 协议基础版本号。
///
/// 与内核 BaseRevision::new(6) 保持一致：
/// 内核声明 6，引导程序回填自身支持的最高版本，内核取较小者。
pub const LIMINE_BASE_REVISION: u64 = 6;
