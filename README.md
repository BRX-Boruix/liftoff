# liftoff

BORUIX 的引导程序：在 UEFI 环境下读取内核 ELF 并交接控制权。

[English](README.en.md)

## 功能

- 从 EXT2 分区读取 `/boot/kernel`，对应安装模式
- 从 ISO9660 光盘读取 `/boot/kernel`，对应 liveCD 模式
- 将内存映射、帧缓冲、RSDP、SMP 信息交给内核
- 仅支持 x86-64 UEFI
- 当前状态：M8 完成。内核已可真实引导且带 SMP 与模块：liftoff 完成 ELF64 校验
  装载、PIE R_X86_64_RELATIVE 重定位、Limine 语义子集协议交接（BaseRevision/
  HHDM/Memmap/Framebuffer/RSDP/SMP/**Modules**/KernelFile/KernelAddress）、4 级
  大页页表与 ExitBootServices；AP 经 MADT 枚举 + 三段式 trampoline + INIT-SIPI
  启动并停泊轮询 goto_address；模块按需加载（内核声明 ModuleRequest 时）并交付
  `ModuleResponse`。kmain 全链、双核上线与模块交付分别在 QEMU/OVMF 实机验证
  （`boot` / `mod` 变体）。

## 已知限制

- 不支持 BIOS 引导
- 不支持 Multiboot 1/2、Linux、chainload 等其它引导协议
- 不提供启动菜单，启动项固定为单一内核
- 内核 ELF 须为静态链接的 PIE（重定位由 bootloader 处理）

## 构建

```
cargo build --release --target x86_64-unknown-uefi
```

产物为 `target/x86_64-unknown-uefi/release/liftoff.efi`，需放入 FAT 分区的 `EFI/BOOT/` 下。

## 仓库布局

- `src/main.rs` —— UEFI 入口与 M2a 文件读取链
- `src/efi.rs` —— 手写最小 UEFI 绑定，布局有编译期断言
- `src/serial.rs` —— COM1 串口输出，观测主通道
- `src/iso9660.rs` —— ISO9660 只读解析器（块读取经 trait 注入）
- `src/ext2.rs` —— EXT2 只读解析器（同一 trait 缝，布局对齐内核侧取证）
- `src/elf.rs` —— ELF64 装载器（校验 + 连续物理映像 + BSS 清零）
- `tools/elf_oracle.py` —— ELF 装载期望值计算器（与驱动同源规则）
- `tools/mkext2.py` —— 确定性 EXT2 fixture 构建器
- `src/config.rs` —— 内核路径与协议版本常量
- `tools/boottest.ps1` —— QEMU OVMF 真实引导验收脚本（构建、摆 ESP、断言串口）
- `tools/mkiso.py` —— 确定性 ISO9660 fixture 构建器

## 相关项目

- [`kernel`](https://github.com/BRX-Boruix/kernel) —— 被引导的内核
- [`tools`](https://github.com/BRX-Boruix/tools) —— 生成可引导镜像并在 QEMU 中运行
- [`wiki`](https://github.com/BRX-Boruix/wiki) —— 协议规划与里程碑记录

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。