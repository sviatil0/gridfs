# gridfs

> A toy filesystem in safe Rust.

[![CI](https://github.com/sviatil0/gridfs/actions/workflows/ci.yml/badge.svg)](https://github.com/sviatil0/gridfs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.74-orange.svg)](Cargo.toml)

`gridfs` is an in-memory, FAT-style filesystem written in 100% safe Rust. It
re-implements the core ideas you would find in a small Unix file system —
superblock, inode table, free-block bitmap, direct + single-indirect block
pointers — and serialises the whole image to a single file so you can save
and reload it on the host. It ships as a library plus a small CLI
(`gridfs-cli`).

## Features

- Hierarchical directories with `mkdir` / `readdir` / `rmdir`
- File `create` / `read` / `write` / `unlink` with byte-granular API
- Direct + single-indirect block pointer scheme (12 direct + N indirect)
- Free-block bitmap with O(N) allocation and O(1) free
- `fsck`-style consistency check (orphans, bitmap mismatches, double-refs)
- Stable binary image format with `dump_image` / `load_image`
- `gridfs-cli` shell with `mkfs`, `ls`, `cat`, `write`, `rm`, `mkdir`,
  `stat`, `fsck`
- No `unsafe`, no async runtime, no external state

## Quick start

```bash
cargo build --release
alias gridfs-cli=./target/release/gridfs-cli

gridfs-cli mkfs disk.img --block-size 512 --num-blocks 64
# created disk.img (64 blocks of 512 bytes)

gridfs-cli -i disk.img mkdir /docs
gridfs-cli -i disk.img write /docs/hello.txt "hello, gridfs"
gridfs-cli -i disk.img ls /docs
# -    1 hello.txt
gridfs-cli -i disk.img cat /docs/hello.txt
# hello, gridfs
gridfs-cli -i disk.img stat /docs/hello.txt
# inode=1 kind=File size=13 blocks=1 links=1
gridfs-cli -i disk.img fsck
# clean
```

## Library usage

```rust
use gridfs::Fs;

fn main() -> Result<(), gridfs::FsError> {
    let mut fs = Fs::new(512, 64)?;
    fs.mkdir("/docs")?;
    fs.create("/docs/notes.txt")?;
    fs.write("/docs/notes.txt", b"hello, gridfs!")?;
    assert_eq!(fs.read("/docs/notes.txt")?, b"hello, gridfs!");

    // Persist to disk and reload.
    let mut buf = Vec::new();
    fs.dump_image(&mut buf)?;
    let reloaded = gridfs::Fs::load_image(&mut std::io::Cursor::new(buf))?;
    assert_eq!(reloaded.read("/docs/notes.txt")?, b"hello, gridfs!");
    Ok(())
}
```

## On-disk format

A serialized image is a single flat byte stream:

```text
+--------------+----------------+----------------+----------------------+
|  Superblock  |  Inode Table   |  Block Bitmap  |     Data Blocks      |
|   24 bytes   |  256 * 64 B    |  ceil(N/8) B   |   N * block_size B   |
+--------------+----------------+----------------+----------------------+
   MAGIC=GRIDFS01    fixed pool      packed LSB-first   raw user data
```

- **Superblock** holds the magic, `block_size`, `num_blocks`, `num_inodes`
  and `root_inode`.
- **Inode table** is a fixed pool of 256 inodes. Each inode stores 12 direct
  block pointers and one single-indirect pointer.
- **Block bitmap** has one bit per data block, packed LSB-first.
- **Data area** is the raw byte storage, addressed by 0-based block index.

## Architecture

```text
+------------------+
|       Fs         |   public API: create / read / write / mkdir / ...
+--------+---------+
         |
         v
+------------------+
|      Image       |   superblock + inode table + bitmap + data
+--------+---------+
         |
   +-----+--------+
   |              |
+--+--+        +--+--+
|inode|        |block|
+-----+        +-----+
```

Domain types live in the library; the CLI is a thin wrapper that reads,
mutates, and rewrites the image file per invocation.

## Limitations

- **Single-threaded.** There is no internal locking.
- **No journaling.** Crashes mid-write can leave the image inconsistent;
  re-run `fsck` to inspect.
- **Fixed inode count** (256). Larger images need a recompile.
- **Max file size** = `12 * block_size + (block_size / 4) * block_size`,
  i.e. a 4 KiB block gives ~4.05 MiB per file.
- **No symlinks, no permissions, no atime/mtime/ctime tracking.**

## Benchmarks

Run `cargo bench` to measure write, read, and directory-walk throughput on
your own machine; results vary by hardware.

## Building & testing

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo bench
```

MSRV is Rust 1.74. CI runs on stable and nightly.

## License

[MIT](LICENSE) — Copyright 2026 Stefan Oleksiienko.
