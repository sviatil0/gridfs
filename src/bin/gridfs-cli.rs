//! `gridfs-cli` — a small command-line shell around the [`gridfs`] library.
//!
//! The CLI loads or creates a `.img` file via the image serializer, runs one
//! command, and writes the image back to disk. Use `mkfs` first to create a
//! fresh image; every subsequent command needs `-i <image>`.
//!
//! Examples:
//!
//! ```bash
//! gridfs-cli mkfs test.img
//! gridfs-cli -i test.img mkdir /docs
//! gridfs-cli -i test.img write /docs/hello.txt "hi"
//! gridfs-cli -i test.img ls /docs
//! gridfs-cli -i test.img cat /docs/hello.txt
//! ```

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use gridfs::{Fs, FsError};

#[derive(Parser)]
#[command(
    name = "gridfs-cli",
    version,
    about = "Operate on gridfs filesystem images"
)]
struct Cli {
    /// Path to the on-disk image file.
    #[arg(short = 'i', long = "image", global = true)]
    image: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new image with the given geometry.
    Mkfs {
        /// Path to the new image file.
        path: PathBuf,
        /// Block size in bytes.
        #[arg(long, default_value_t = 512)]
        block_size: usize,
        /// Number of data blocks.
        #[arg(long, default_value_t = 256)]
        num_blocks: usize,
    },
    /// List the contents of a directory.
    Ls {
        /// Directory path.
        #[arg(default_value = "/")]
        path: String,
    },
    /// Read a file and print it to stdout.
    Cat {
        /// File path.
        path: String,
    },
    /// Write a string to a file, creating it if needed.
    Write {
        /// File path.
        path: String,
        /// Contents to write.
        contents: String,
    },
    /// Remove a regular file.
    Rm {
        /// File path.
        path: String,
    },
    /// Create a new directory.
    Mkdir {
        /// Directory path.
        path: String,
    },
    /// Show metadata about a path.
    Stat {
        /// File or directory path.
        path: String,
    },
    /// Run consistency checks.
    Fsck,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gridfs-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), FsError> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Mkfs {
            path,
            block_size,
            num_blocks,
        } => {
            let fs = Fs::new(block_size, num_blocks)?;
            write_image(&path, &fs)?;
            println!(
                "created {} ({} blocks of {} bytes)",
                path.display(),
                num_blocks,
                block_size
            );
            Ok(())
        }
        Cmd::Ls { path } => {
            let img = require_image(&cli.image)?;
            let fs = read_image(&img)?;
            let entries = fs.readdir(&path)?;
            for e in entries {
                let tag = match e.kind {
                    gridfs::FileKind::Directory => "d",
                    gridfs::FileKind::File => "-",
                };
                println!("{tag} {:>4} {}", e.inode, e.name);
            }
            Ok(())
        }
        Cmd::Cat { path } => {
            let img = require_image(&cli.image)?;
            let fs = read_image(&img)?;
            let data = fs.read(&path)?;
            std::io::stdout().write_all(&data).map_err(FsError::from)?;
            Ok(())
        }
        Cmd::Write { path, contents } => {
            let img = require_image(&cli.image)?;
            let mut fs = read_image(&img)?;
            if fs.stat(&path).is_err() {
                fs.create(&path)?;
            }
            fs.write(&path, contents.as_bytes())?;
            write_image(&img, &fs)?;
            Ok(())
        }
        Cmd::Rm { path } => {
            let img = require_image(&cli.image)?;
            let mut fs = read_image(&img)?;
            fs.unlink(&path)?;
            write_image(&img, &fs)?;
            Ok(())
        }
        Cmd::Mkdir { path } => {
            let img = require_image(&cli.image)?;
            let mut fs = read_image(&img)?;
            fs.mkdir(&path)?;
            write_image(&img, &fs)?;
            Ok(())
        }
        Cmd::Stat { path } => {
            let img = require_image(&cli.image)?;
            let fs = read_image(&img)?;
            let stat = fs.stat(&path)?;
            println!(
                "inode={} kind={:?} size={} blocks={} links={}",
                stat.inode, stat.kind, stat.size, stat.blocks, stat.link_count
            );
            Ok(())
        }
        Cmd::Fsck => {
            let img = require_image(&cli.image)?;
            let fs = read_image(&img)?;
            let report = fs.fsck();
            if report.is_clean() {
                println!("clean");
            } else {
                for issue in &report.issues {
                    println!("{issue}");
                }
            }
            Ok(())
        }
    }
}

fn require_image(image: &Option<PathBuf>) -> Result<PathBuf, FsError> {
    image
        .clone()
        .ok_or_else(|| FsError::InvalidPath("--image is required for this command".into()))
}

fn read_image(path: &PathBuf) -> Result<Fs, FsError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    Fs::load_image(&mut reader)
}

fn write_image(path: &PathBuf, fs: &Fs) -> Result<(), FsError> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    fs.dump_image(&mut writer)?;
    writer.flush()?;
    Ok(())
}
