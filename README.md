# Liftoff

BORUIX 的引导程序。UEFI 环境下加载内核并交接控制权。

名字取自火箭离地那一刻——引导程序的全部职责，就是把内核送上去。

## 定位

这不是一个通用 bootloader，而是**只为 BORUIX 服务**的引导程序。

明确不做的：

- 多启动协议（Multiboot 1/2、chainload、Linux/Windows 直接引导）
- 多架构（仅 x86-64 UEFI）
- 图形菜单与主题
- PXE / TFTP
- FAT32 驱动（UEFI 固件已提供，无需自写）

明确要做的：

| 能力 | 说明 |
|---|---|
| EXT2 读取 | 安装模式：从 EXT2 根分区读 `/boot/kernel` |
| ISO9660 读取 | liveCD 模式：从光盘镜像读 `/boot/kernel` |
| ELF 加载 | 解析并加载 x86-64 静态 ELF 内核 |
| 内存映射透传 | UEFI `GetMemoryMap` → 引导协议 memmap |
| 协议响应构造 | 填充内核所需的请求响应结构 |
| 交接 | 建立页表、跳转内核入口 |

## 两种启动模式

与内核 `boot_source()` 的判定保持一致，靠 `media_type` 区分：

| 模式 | 介质 | 文件系统 | 内核看到的 media_type |
|---|---|---|---|
| liveCD | ISO 光盘 | ISO9660 | `1` (optical) |
| 安装模式 | 磁盘 | EXT2 | `0` (generic) |

两种模式都必须支持——liveCD 是默认启动方式（`tools/tools_build/run.py` 默认
`-cdrom` + `boot_order=d`）。

## 引导协议：两步走

Liftoff 与内核之间的交接契约，**分两个阶段实施**。

### 阶段一：复用 Limine 协议（当前）

内核现在用 `brxlimine-rs` 解析 Limine 协议结构，耦合面只有 **26 行、7 个文件**：

| 文件 | 用到的请求 |
|---|---|
| `kernel/crates/kernel/src/main.rs` | `BaseRevision`、`Framebuffer`、`KernelFile` |
| `kernel/crates/kernel/src/acpi.rs` | `Rsdp` |
| `kernel/crates/kernel/src/smp.rs` | `Smp` |
| `kernel/crates/kernel/src/drivers.rs` | `Framebuffer`（消费帧缓冲） |

阶段一 Liftoff 只实现这 5 个请求，内核**零改动**。

**为什么先走这条路**：本阶段同时引入四件新事物——UEFI 引导、EXT2、ISO9660、
ELF 加载。若再叠加自拟协议，任何异常都无法定位是哪一层的锅。复用 Limine 协议
还能保留一个对照物：用 `brxLimine` 跑一遍作基线，可快速区分"引导程序错了"
还是"内核错了"。

### 阶段二：自拟协议（待启动）

阶段一稳定运行后，替换为 BORUIX 自有协议。

**动机**：

1. Limine 协议有 40+ 请求，本项目只用 5 个，其余为纯负担
2. Limine 是 C ABI，强制 `#[repr(C)]` + 手写 `Ptr<T>`/`NonNullPtr<T>` 包装，
   `brxlimine-rs` 中大半代码在此
3. `BaseRevision` 版本协商为跨版本兼容而设，本项目引导程序与内核同仓发布，
   版本恒匹配，无需协商

**设计约束**（不可协商）：

1. **不做多版本协商**：只保留 `magic` + `version` 两个校验字段，做精确匹配；
   版本不符即拒绝启动，不兼容多版本
2. **不手写指针包装**：协议结构用 Rust 原生类型（`Option<&T>`、`&[T]`）定义，
   仅在交接那一刻转为裸地址
3. **必须写"谁负责填"清单**：协议文档的核心价值是明确每个字段由引导程序填、
   还是由内核消费、还是双方约定不可触碰。Limine `PROTOCOL.md` 中最有价值
   的部分正是这份职责划分
4. **保留 brxLimine 作为回归基线**，直到阶段二稳定

**待定项**（设计时需明确）：

- 内存映射的编码方式（类型枚举、保留位语义）
- 高半区直接映射（HHDM）地址约定——内核 `mm` 强依赖
- 页表交接方式——内核是否重建页表
- SMP 启动 AP 的握手协议
- 帧缓冲像素格式枚举

## 为什么不用 brxLimine

`brxLimine`（Limine 12.5.2 fork）功能远超 BORUIX 所需：约 10 万行源码，而我们只用到
其中的 5 个协议请求、EXT2 与 ISO9660 加载。翻译或沿用它等于把 95% 用不到的功能
一起背负，且永远追不上上游。

详见 BORUIX 仓库的 ADR 记录。

## 构建

```
cargo build --release --target x86_64-unknown-uefi
```

产物：`target/x86_64-unknown-uefi/release/liftoff.efi`

## 状态

骨架阶段。已建立工程结构并验证 UEFI 目标可编译（产物为合法 PE/COFF：
`Machine 0x8664`、`Subsystem 10`）。尚未实现功能。
