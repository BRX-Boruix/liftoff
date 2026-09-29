# liftoff

BORUIX 的引导程序：在 UEFI 环境下读取内核 ELF 并交接控制权。

[English](README.en.md)

## 功能

- 从 EXT2 分区读取 `/boot/kernel`，对应安装模式
- 从 ISO9660 光盘读取 `/boot/kernel`，对应 liveCD 模式
- 将内存映射、帧缓冲、RSDP、SMP 信息交给内核
- 仅支持 x86-64 UEFI
- 当前状态：M2a 完成。UEFI 文件协议链（LoadedImage → SimpleFileSystem → Open → Read）
  已在 QEMU OVMF 下验证，能读取引导介质上的文件并报告长度与校验和；
  EXT2、ISO9660、ELF 加载尚未实现

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
- `src/config.rs` —— 内核路径与协议版本常量
- `tools/boottest.ps1` —— QEMU OVMF 真实引导验收脚本（构建、摆 ESP、断言串口）

## 相关项目

- [`kernel`](https://github.com/BRX-Boruix/kernel) —— 被引导的内核
- [`tools`](https://github.com/BRX-Boruix/tools) —— 生成可引导镜像并在 QEMU 中运行
- [`wiki`](https://github.com/BRX-Boruix/wiki) —— 协议规划与里程碑记录

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。
