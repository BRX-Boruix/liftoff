# Liftoff (gen2)

BORUIX bootloader, gen2: a clean-room rewrite in Rust that follows the semantics
of brxLimine (the project fork of Limine) item by item.

## Goal

- Boot the BORUIX kernel from UEFI through a Limine-protocol subset.
- Match brxLimine on AP bring-up semantics (GDT/TSS, IA32_APIC_BASE, MTRR, LAPIC
  handoff state, iretq entry with zeroed GPRs).
- Stay decoupled from any specific kernel: depend only on the Limine protocol and
  the x86 architecture definitions.

## Layout (by responsibility)

| Layer | Path | Responsibility |
| --- | --- | --- |
| Entry | `src/main.rs` | UEFI entry point, panic handler, halt |
| Firmware bindings | `src/efi.rs` | EFI types and protocols, expanded on demand |
| Debug output | `src/serial.rs` | COM1 console |

Later stages add `arch/` (cpu, gdt, lapic, smp, trampoline), `mm/` (paging,
memmap, mtrr), `protos/` (Limine requests) and `loader/` (ELF, filesystems).

## Build

```
cargo build --release --target x86_64-unknown-uefi
```

Artifact: `target/x86_64-unknown-uefi/release/liftoff.efi`.
