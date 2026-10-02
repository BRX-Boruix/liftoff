# liftoff

> **Deprecated (2026-10-02)**: liftoff is no longer developed, and BORUIX no longer supports it.
> The UEFI boot path is unmaintained; BORUIX keeps only brxLimine (BIOS) boot. The supporting
> tooling and docs have moved to the `liftoff/` directory of the
> [archive](https://github.com/BRX-Boruix/archive) repository; this repository is kept for reference.

BORUIX's UEFI bootloader: it loads the kernel and hands over to it following the Limine protocol.

[中文](README.md)

## Purpose

Start from UEFI firmware, read the kernel image, fill the Limine requests the kernel declares,
exit boot services and jump to the kernel entry point.

The repository currently holds only the cargo project skeleton; the boot logic is not implemented.

## Known limitations

- No boot logic yet: there is no UEFI entry point, so the build fails before linking
- x86_64 only

## Build

```
cargo build --release --target x86_64-unknown-uefi
```

Artifact: `target/x86_64-unknown-uefi/release/liftoff.efi`. `rust-toolchain.toml` pins nightly and
installs `rust-src`, `llvm-tools` and the `x86_64-unknown-uefi` target.

## Repository layout

- `crates/boot` -- the executable (binary name `liftoff`): UEFI entry, orchestration and wiring
- `crates/protocol/limine` -- Limine protocol contract and request handling
- `crates/arch/arch`, `crates/arch/x86_64` -- architecture abstraction and the x86_64 implementation
- `crates/mm`, `crates/fs`, `crates/driver`, `crates/loader`, `crates/utils` -- architecture-independent layers
- `crates/efi` -- UEFI types and protocol bindings
- `crates/flanterm_rust` -- vendored framebuffer terminal (MIT + BSD-2)

- `src/main.rs` -- program entry

## Related projects

- [`kernel`](https://github.com/BRX-Boruix/kernel) -- the kernel it loads
- [`tools`](https://github.com/BRX-Boruix/tools) -- build and acceptance

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).

Third-party components bundled in this repository are listed in [NOTICE.md](NOTICE.md).
