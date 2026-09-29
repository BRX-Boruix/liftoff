# liftoff

BORUIX 的引导程序：在 UEFI 环境下读取内核 ELF 并交接控制权。

[English](README.en.md)

## 功能

- 从 EXT2 分区读取 `/boot/kernel`，对应安装模式
- 从 ISO9660 光盘读取 `/boot/kernel`，对应 liveCD 模式
- 将内存映射、帧缓冲、RSDP、SMP 信息交给内核
- 仅支持 x86-64 UEFI
- 当前状态：M2c 完成。ISO9660 与 EXT2 只读驱动均已实现并经 QEMU OVMF 真实设备
  验证（光盘与磁盘双链路：块设备枚举 → 探测挂载 → 目录遍历 → 文件读取）；
  ELF 加载尚未实现

## 已知限制

- 不支持 BIOS 引导
- 不支持 Multiboot 1/2、Linux、chainload 等其它引导协议
- 不提供启动菜单，启动项固定为单一内核
- 内核 ELF 须为静态链接

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