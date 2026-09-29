# liftoff

BORUIX's bootloader: reads the kernel ELF under UEFI and hands over control.

[简体中文](README.md)

## Features

- Reads `/boot/kernel` from an EXT2 partition (install mode)
- Reads `/boot/kernel` from an ISO9660 disc (liveCD mode)
- Passes the memory map, framebuffer, RSDP and SMP information to the kernel
- x86-64 UEFI only
- Current status: M2b done. A read-only ISO9660 driver is implemented and verified against a real
  optical device under QEMU OVMF (block device enumeration, CD001 probe, directory traversal,
  file read); EXT2 and ELF loading are not yet implemented

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

- `src/main.rs` — UEFI entry point and the M2a file read chain
- `src/efi.rs` — hand-written minimal UEFI bindings with compile-time layout assertions
- `src/serial.rs` — COM1 serial output, the primary observation channel
- `src/iso9660.rs` — read-only ISO9660 parser (block reads injected via a trait)
- `src/config.rs` — kernel path and protocol version constants
- `tools/boottest.ps1` — QEMU OVMF real-boot acceptance script (build, stage ESP, assert serial log)
- `tools/mkiso.py` — deterministic ISO9660 fixture builder

## Related projects

- [`kernel`](https://github.com/BRX-Boruix/kernel) — the kernel it boots
- [`tools`](https://github.com/BRX-Boruix/tools) — builds bootable images and runs them under QEMU
- [`wiki`](https://github.com/BRX-Boruix/wiki) — protocol planning and milestone records

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).