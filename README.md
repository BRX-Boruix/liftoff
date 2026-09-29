# Liftoff

BORUIX 的引导程序。UEFI 环境下通过 **Limine 引导协议**加载内核。

名字取自火箭离地那一刻——引导程序的全部职责，就是把内核送上去。

## 定位

这不是一个通用 bootloader，而是**只为 BORUIX 服务**的引导程序。

明确不做的：

- 多启动协议（Multiboot 1/2、chainload、Linux/Windows 直接引导）
- 多架构（仅 x86-64 UEFI）
- 图形菜单与主题
- PXE / TFTP
- ISO9660、FAT32 支持

明确要做的：

| 能力 | 说明 |
|---|---|
| EXT2 读取 | 从 EXT2 根分区读 `/boot/kernel` |
| ELF 加载 | 解析并加载 x86-64 静态 ELF 内核 |
| 内存映射透传 | UEFI `GetMemoryMap` → Limine memmap |
| Limine 协议 | 填充内核所需的请求响应结构 |
| 交接 | 建立页表、跳转内核入口 |

## 内核所需的协议请求

Liftoff 只需实现 BORUIX 内核实际用到的 5 个：

1. `BaseRevision`
2. `Framebuffer`
3. `KernelFile`
4. `Rsdp`
5. `Smp`

协议结构定义以 `limine-protocol/PROTOCOL.md` 为唯一规范，**必须字节精确**。

## 为什么不用 brxLimine

`brxLimine`（Limine 12.5.2 fork）功能远超 BORUIX 所需：约 10 万行源码，而我们只用到其中
的 5 个协议请求和 EXT2 加载。翻译它等于把 95% 用不到的功能一起搬过来，且永远追不上上游。

详见 BORUIX 仓库的 ADR 记录。

## 构建

```
cargo build --release --target x86_64-unknown-uefi
```

产物：`target/x86_64-unknown-uefi/release/liftoff.efi`

## 状态

骨架阶段，尚未实现功能。
