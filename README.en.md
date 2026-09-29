# liftoff

BORUIX's bootloader: reads the kernel ELF under UEFI and hands over control.

[简体中文](README.md)

## Features

- Reads `/boot/kernel` from an ISO9660 disc (liveCD mode)
- Reads `/boot/kernel` from EXT2 inside an MBR partition (install mode)
- Hands the kernel a Limine-semantic protocol subset: base revision, HHDM, memory map,
  framebuffer, RSDP, SMP, modules, kernel file and kernel address
- Starts every auxiliary processor (AP) and delivers the Limine module request
- x86-64 UEFI only

## Status

M11 is done. Both boot chains, modules and multi-core are verified end to end under
QEMU/OVMF (a 16-variant regression suite):

- **liveCD** (ISO9660): the `boot` variant - the full kernel chain;
- **install mode** (MBR partition + EXT2): the `ext-boot` variant - reads the 24.6 MiB
  kernel from disk (double indirect blocks); the kernel identifies the boot disk from the
  `mbr_disk_id`/`partition_index` we filled and mounts that EXT2 partition as root;
- **modules**: `mod` (ISO) and `ext-mod` (EXT2) - one module list and one assembler,
  loaded on demand only when the kernel declares `ModuleRequest`;
- **multi-core**: the `smp4` variant (`-smp 4`) - all three APs are started by liftoff and
  taken over by the kernel.

x2APIC needs matching kernel-side work (the kernel LAPIC is MMIO-only today), so xAPIC is
used for now.

## Known limitations

- No BIOS boot
- No Multiboot 1/2, Linux, or chainload support
- No boot menu; the boot entry is a single fixed kernel
- The kernel ELF must be a statically linked PIE (relocations handled by the bootloader)

## Building

```
cargo build --release --target x86_64-unknown-uefi
```

The artifact is `target/x86_64-unknown-uefi/release/liftoff.efi`; place it as
`EFI/BOOT/BOOTX64.EFI` on a FAT partition.

## Verification

`tools/boottest.ps1` really boots the image under QEMU + OVMF and asserts on serial
output - an assertion only passes if the firmware actually loaded liftoff and liftoff
actually brought the kernel up.

Requirements: `pwsh`, Python 3, QEMU (including `share/edk2-x86_64-code.fd`), and
`QEMU_DIR` set in a `.env` file at the workspace root.

```
pwsh tools/boottest.ps1 -Variant boot      # liveCD, full chain (-smp 2)
pwsh tools/boottest.ps1 -Variant smp4      # four CPUs online
pwsh tools/boottest.ps1 -Variant mod       # modules over ISO
pwsh tools/boottest.ps1 -Variant ext-mod   # modules over EXT2
pwsh tools/boottest.ps1 -Variant ext-boot  # install mode, full chain (needs systemdisk.img)
pwsh tools/boottest.ps1 -Variant elf-iso   # ELF load contract (expectations from elf_oracle.py)
```

Some variants need external artifacts:

- `boot` / `smp4`: `target/kernel.elf` (a BORUIX kernel build)
- `ext-boot`: `systemdisk.img` at the workspace root (produced by
  `python main.py build --systemdisk` in the [`tools`](https://github.com/BRX-Boruix/tools) repo)
- `mod` / `ext-mod`: the script builds `tools/modtest` on the spot with `rustc`
  (needs the `x86_64-unknown-none` target)

## Layout

- `src/main.rs` - UEFI entry, the per-milestone boot chains and the handover orchestration
- `src/efi.rs` - hand-written minimal UEFI bindings (compile-time layout assertions)
- `src/serial.rs` - COM1 output, the main observation channel
- `src/iso9660.rs` - read-only ISO9660 parser plus the BlockIo adapter (chunked reads, media bounds)
- `src/ext2.rs` - read-only EXT2 parser (partition offset; direct / single / double indirect)
- `src/elf.rs` - ELF64 loader (validation, one contiguous image, BSS zeroing, PIE relocations)
- `src/paging.rs` - 4-level huge-page tables (identity + HHDM + kernel high window)
- `src/boruix.rs` - protocol constants/structs and the request-tag scan
- `src/handover.rs` - ExitBootServices, memory-map conversion and the jump
- `src/smp.rs` - MADT enumeration, the three-stage AP trampoline, INIT-SIPI
- `src/modules.rs` - module assembly (shared by ISO and EXT2)
- `src/config.rs` - kernel paths, protocol revision and the module list
- `tools/boottest.ps1` - QEMU + OVMF boot acceptance (16 variants)
- `tools/mkiso.py` / `tools/mkext2.py` - deterministic ISO9660 / EXT2 fixture builders
- `tools/elf_oracle.py` / `tools/modtest_oracle.py` - expectation calculators (same rules as the drivers)
- `tools/modtest/` - the standalone consumer kernel used by the module acceptance

## Related projects

- [`kernel`](https://github.com/BRX-Boruix/kernel) - the kernel that gets booted
- [`tools`](https://github.com/BRX-Boruix/tools) - builds bootable images and runs them in QEMU
- [`wiki`](https://github.com/BRX-Boruix/wiki) - protocol planning and milestone records

## License

MIT License, copyright (c) Yang Borui. See [LICENSE](LICENSE).
