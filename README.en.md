# liftoff

BORUIX's bootloader: reads the kernel ELF under UEFI and hands over control.

[简体中文](README.md)

## Features

- Reads `/boot/kernel` from an EXT2 partition (install mode)
- Reads `/boot/kernel` from an ISO9660 disc (liveCD mode)
- Passes the memory map, framebuffer, RSDP and SMP information to the kernel
- x86-64 UEFI only

## Known limitations

- No BIOS boot
- No Multiboot 1/2, Linux, or chainload support
- No boot menu; the boot entry is a single fixed kernel
- The kernel ELF must be statically linked

## Building

```
cargo build --release --target x86_64-unknown-uefi
```

The output is `target/x86_64-unknown-uefi/release/liftoff.efi`, which goes under `EFI/BOOT/` on a FAT partition.

## Repository layout

- `src/main.rs` — UEFI entry point
- `src/config.rs` — kernel path and protocol version constants

## Related projects

- [`kernel`](https://github.com/BRX-Boruix/kernel) — the kernel it boots
- [`tools`](https://github.com/BRX-Boruix/tools) — builds bootable images and runs them under QEMU

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).
## Status

M1 done: entry signature check and dual-channel serial/console output verified under QEMU OVMF. Filesystem and kernel loading not yet implemented.
