# liftoff

BORUIX's UEFI bootloader: it loads the kernel and hands over to it following the Limine protocol.

[中文](README.md)

## Purpose

Start from UEFI firmware, read the kernel image, fill the Limine requests the kernel declares,
exit boot services and jump to the kernel entry point.

The gen2 rewrite is in progress: only the UEFI entry point and COM1 output exist today, the kernel
is not loaded yet.

## Known limitations

- The current version does not load a kernel: it initialises COM1, writes one line and returns to firmware
- x86_64 only

## Build

```
cargo build --release --target x86_64-unknown-uefi
```

Artifact: `target/x86_64-unknown-uefi/release/liftoff.efi`. `rust-toolchain.toml` pins nightly and
installs `rust-src`, `llvm-tools` and the `x86_64-unknown-uefi` target.

## Repository layout

- `src/main.rs` -- UEFI entry point, panic handler and halt
- `src/efi.rs` -- EFI types and protocols
- `src/serial.rs` -- COM1 output

## Related projects

- [`kernel`](https://github.com/BRX-Boruix/kernel) -- the kernel it loads
- [`tools`](https://github.com/BRX-Boruix/tools) -- build and acceptance

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).
