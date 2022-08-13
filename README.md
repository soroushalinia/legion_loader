# Bootloader for Legion OS

UEFI Bootloader for Legion kernel. It is very simple at the moment and it only loads kernel into memory and call `_start()` in kernel which is kernel entry point. Then it passes framebuffer and memory map to the kernel.

# Build

Use cargo to build:

```bash
cargo build --release
```

Compiled .efi file is at `target/x86_64-unknown-uefi/release/legion_loader.efi`

# How to use

Create a FAT/FAT16/FAT32 partition.

Copy and change file name to following in the root of partition:

`Boot\EFI\BOOTX64.EFI`
