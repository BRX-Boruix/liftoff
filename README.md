# liftoff

BORUIX 的 UEFI 引导程序：按 Limine 协议加载内核并交接给内核。

[English](README.en.md)

## 用途

从 UEFI 固件启动，读取内核映像，填写内核声明的 Limine 请求，退出引导服务后跳转内核入口。

gen2 重写进行中：当前只有 UEFI 入口与 COM1 串口输出，尚未加载内核。

## 已知限制

- 当前版本不加载内核，启动后初始化 COM1 并输出一行信息后返回固件
- 只支持 x86_64

## 构建

```
cargo build --release --target x86_64-unknown-uefi
```

产物是 `target/x86_64-unknown-uefi/release/liftoff.efi`。工具链由 `rust-toolchain.toml` 固定为
nightly，并安装 `rust-src`、`llvm-tools` 与 `x86_64-unknown-uefi` 目标。

## 仓库布局

- `src/main.rs` —— UEFI 入口、panic 处理与 halt
- `src/efi.rs` —— EFI 类型与协议
- `src/serial.rs` —— COM1 串口输出

## 相关项目

- [`kernel`](https://github.com/BRX-Boruix/kernel) —— 被加载的内核
- [`tools`](https://github.com/BRX-Boruix/tools) —— 构建与验收

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。
