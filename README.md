# liftoff

BORUIX 的引导程序：在 UEFI 环境下读取内核 ELF 并交接控制权。

[English](README.en.md)

## 功能

- 从 ISO9660 光盘读取 `/boot/kernel`（liveCD 模式）
- 从 MBR 分区内的 EXT2 读取 `/boot/kernel`（安装模式）
- 向内核交接 Limine 语义子集协议：BaseRevision、HHDM、内存映射、帧缓冲、
  RSDP、SMP、模块、内核文件、内核地址
- 启动全部辅助处理器（AP）并交付 Limine 的模块请求
- 仅支持 x86-64 UEFI

## 当前状态

M12 完成。两条启动链、模块、多核与 x2APIC 均已在 QEMU/OVMF 上端到端验证：

- **liveCD**（ISO9660）：`boot` 变体 —— 内核全链；
- **安装模式**（MBR 分区 + EXT2）：`ext-boot` 变体 —— 读取盘上 24.6 MB 内核
  （含二级间接块），内核据 liftoff 填的 `mbr_disk_id`/`partition_index` 识别启动盘，
  并把该 EXT2 分区挂为根；
- **模块**：`mod`（ISO）与 `ext-mod`（EXT2）—— 同一份清单、同一装配逻辑，
  仅当内核声明 `ModuleRequest` 时按需加载；
- **多核**：`smp4` 变体（`-smp 4`）—— 3 个 AP 全部由 liftoff 启动并被内核接管；
- **x2APIC**：`x2apic` 变体（`-cpu qemu64,+x2apic -smp 4`）—— CPU 支持时切到 MSR
  访问、以 `SmpResponse.flags` bit0 告知内核，4 核仍全部上线；默认 CPU 无 x2APIC 时
  自动回退 MMIO（`smp4` 变体即覆盖回退路径）。


## 已知限制

- 不支持 BIOS 引导
- 不支持 Multiboot 1/2、Linux、chainload 等其它引导协议
- 不提供启动菜单，启动项固定为单一内核
- 内核 ELF 须为静态链接的 PIE（重定位由 bootloader 处理）

## 构建

```
cargo build --release --target x86_64-unknown-uefi
```

产物为 `target/x86_64-unknown-uefi/release/liftoff.efi`，需放入 FAT 分区的
`EFI/BOOT/BOOTX64.EFI`。

## 验收

`tools/boottest.ps1` 在 QEMU + OVMF 上真实引导并断言串口输出——只有固件真的装载了
liftoff、liftoff 真的把内核送起来，断言才会通过。

前置：`pwsh`、Python 3、QEMU（含 `share/edk2-x86_64-code.fd`），并在工作区根目录的
`.env` 中设置 `QEMU_DIR`。

```
pwsh tools/boottest.ps1 -Variant boot      # liveCD 全链（-smp 2）
pwsh tools/boottest.ps1 -Variant smp4      # 4 核全部上线（xAPIC 回退路径）
pwsh tools/boottest.ps1 -Variant x2apic    # 4 核 + x2APIC（-cpu qemu64,+x2apic）
pwsh tools/boottest.ps1 -Variant mod       # 模块（ISO）
pwsh tools/boottest.ps1 -Variant ext-mod   # 模块（EXT2）
pwsh tools/boottest.ps1 -Variant ext-boot  # 安装模式全链（需 systemdisk.img）
pwsh tools/boottest.ps1 -Variant elf-iso   # ELF 装载契约（期望值由 elf_oracle.py 同源计算）
```

部分变体需要外部产物：

- `boot` / `smp4`：`target/kernel.elf`（BORUIX 内核的构建产物）
- `ext-boot`：**无需外部产物** —— 脚本用 `tools/mksysdisk.py` 自建系统盘（MBR 磁盘签名
  0x424F5255、分区 1 起始 LBA 2048、EXT2 内 `/boot/kernel`），布局与项目工具链产出的
  `systemdisk.img` 逐字段一致
- `mod` / `ext-mod`：脚本会用 `rustc` 现场构建 `tools/modtest`（需 `x86_64-unknown-none` 目标）

## 仓库布局

- `src/main.rs` —— UEFI 入口、各里程碑启动链与交接编排
- `src/efi.rs` —— 手写最小 UEFI 绑定（布局有编译期断言）
- `src/serial.rs` —— COM1 串口输出，观测主通道
- `src/iso9660.rs` —— ISO9660 只读解析器 + BlockIo 适配（分块读、媒体边界保护）
- `src/ext2.rs` —— EXT2 只读解析器（分区偏移；直块 / 一级 / 二级间接）
- `src/elf.rs` —— ELF64 装载器（校验 + 连续物理映像 + BSS 清零 + PIE 重定位）
- `src/paging.rs` —— 4 级大页页表（恒等 + HHDM + 内核高区）
- `src/boruix.rs` —— 协议常量/结构与请求标记扫描
- `src/handover.rs` —— ExitBootServices、内存映射转换与跳转
- `src/smp.rs` —— MADT 枚举、三段式 AP trampoline、INIT-SIPI
- `src/modules.rs` —— 模块装配（ISO/EXT2 共用）
- `src/config.rs` —— 内核路径、协议版本与模块清单常量
- `tools/boottest.ps1` —— QEMU + OVMF 真实引导验收（16 个变体）
- `tools/mkiso.py` / `tools/mkext2.py` —— 确定性 ISO9660 / EXT2 fixture 构建器
- `tools/elf_oracle.py` / `tools/modtest_oracle.py` —— 期望值计算（与驱动同源规则）
- `tools/modtest/` —— 模块协议验收用的独立消费者内核

## 相关项目

- [`kernel`](https://github.com/BRX-Boruix/kernel) —— 被引导的内核
- [`tools`](https://github.com/BRX-Boruix/tools) —— 生成可引导镜像并在 QEMU 中运行
- [`wiki`](https://github.com/BRX-Boruix/wiki) —— 协议规划与里程碑记录

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。
