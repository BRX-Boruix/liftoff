# liftoff

BORUIX 的 UEFI 引导程序：按 Limine 协议加载内核并交接给内核。

[English](README.en.md)

## 用途

从 UEFI 固件启动，读取内核映像，填写内核声明的 Limine 请求，退出引导服务后跳转内核入口。

当前仓库只有 cargo 建立的项目骨架，引导逻辑尚未实现。

## 已知限制

- 引导逻辑尚未实现，当前没有 UEFI 入口，构建在链接阶段之前即失败
- 只支持 x86_64

## 构建

```
cargo build --release --target x86_64-unknown-uefi
```

产物是 `target/x86_64-unknown-uefi/release/liftoff.efi`。工具链由 `rust-toolchain.toml` 固定为
nightly，并安装 `rust-src`、`llvm-tools` 与 `x86_64-unknown-uefi` 目标。

## 仓库布局

- `crates/boot` —— 可执行产物（bin 名 `liftoff`）：UEFI 入口、编排与装配
- `crates/protocol/limine` —— Limine 协议契约与请求处理
- `crates/arch/arch`、`crates/arch/x86_64` —— 架构抽象与 x86_64 实现
- `crates/mm`、`crates/fs`、`crates/driver`、`crates/loader`、`crates/utils` —— 与架构无关的层
- `crates/efi` —— UEFI 类型与协议绑定
- `crates/flanterm_rust` —— vendor 的 framebuffer 终端（MIT + BSD-2）

- `src/main.rs` —— 程序入口

## 相关项目

- [`kernel`](https://github.com/BRX-Boruix/kernel) —— 被加载的内核
- [`tools`](https://github.com/BRX-Boruix/tools) —— 构建与验收

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。
