//! # gridfs
//!
//! A small, in-memory, FAT-style filesystem written in safe Rust.
//!
//! `gridfs` is intended as a teaching tool and portfolio piece. It models the
//! core ideas of a classic Unix filesystem — superblock, inode table, free
//! block bitmap, and a data area addressed by direct + single-indirect block
//! pointers — without any of the OS plumbing required by a real driver.
//!
//! ## Quick start
//!
//! ```
//! use gridfs::Fs;
//!
//! let mut fs = Fs::new(512, 64).unwrap();
//! fs.mkdir("/docs").unwrap();
//! fs.create("/docs/notes.txt").unwrap();
//! fs.write("/docs/notes.txt", b"hello, gridfs!").unwrap();
//!
//! let data = fs.read("/docs/notes.txt").unwrap();
//! assert_eq!(data, b"hello, gridfs!");
//! ```
//!
//! See [`Fs`] for the full public surface.

#![deny(missing_docs)]

pub mod block;
pub mod error;
pub mod inode;
pub mod path;

use std::io::{Read, Write};

pub use error::{FsError, Result};
pub use inode::{FileKind, Inode, InodeId};

use block::{DirEntry, Image, Superblock, DIR_ENTRY_SIZE, INODE_COUNT, MAGIC};
use inode::{DIRECT_POINTERS, NULL_BLOCK};
use path::{split_parent, split_path};

/// Result of `stat()` — a snapshot of an inode's metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    /// Inode id of the entry.
    pub inode: InodeId,
    /// Whether the entry is a file or directory.
    pub kind: FileKind,
    /// Logical size in bytes.
    pub size: u64,
    /// Number of hard links pointing at this inode (always 1 today).
    pub link_count: u16,
    /// Number of data blocks used by this entry.
    pub blocks: u32,
}

/// One directory listing item produced by [`Fs::readdir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Filename (last path component).
    pub name: String,
    /// Inode id pointed at by this entry.
    pub inode: InodeId,
    /// Kind of the target inode.
    pub kind: FileKind,
}

/// Outcome of [`Fs::fsck`] — a list of consistency findings.
#[derive(Debug, Default, Clone)]
pub struct FsckReport {
    /// Human-readable consistency issues; empty when the image is clean.
    pub issues: Vec<String>,
}

impl FsckReport {
    /// Returns `true` when the filesystem is consistent.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// The filesystem handle. All public operations go through this type.
pub struct Fs {
    image: Image,
}

impl Fs {
    /// Creates a fresh, empty filesystem with the given geometry.
    ///
    /// `block_size` must be at least 64 bytes; `num_blocks` must be > 0.
    pub fn new(block_size: usize, num_blocks: usize) -> Result<Self> {
        Ok(Self {
            image: Image::new(block_size, num_blocks)?,
        })
    }

    /// Returns the geometry of the underlying image.
    pub fn superblock(&self) -> &Superblock {
        &self.image.sb
    }

    /// Re-initialises the image, discarding all data.
    pub fn format(&mut self) -> Result<()> {
        self.image = Image::new(
            self.image.sb.block_size as usize,
            self.image.sb.num_blocks as usize,
        )?;
        Ok(())
    }

    /// Creates an empty regular file at `path`.
    pub fn create(&mut self, path: &str) -> Result<Inode> {
        let (parent_parts, name) = match split_parent(path)? {
            Some(t) => t,
            None => return Err(FsError::InvalidPath(path.into())),
        };
        let parent = self.resolve_components(&parent_parts)?;
        self.assert_kind(parent, FileKind::Directory, path)?;
        if self.find_entry(parent, &name)?.is_some() {
            return Err(FsError::AlreadyExists(path.into()));
        }
        let new_id = self.image.alloc_inode(FileKind::File)?;
        self.add_dir_entry(parent, &name, new_id)?;
        Ok(self.image.inodes[new_id as usize].clone())
    }

    /// Creates a directory at `path`. The parent directory must already exist.
    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        let (parent_parts, name) = match split_parent(path)? {
            Some(t) => t,
            None => return Err(FsError::AlreadyExists("/".into())),
        };
        let parent = self.resolve_components(&parent_parts)?;
        self.assert_kind(parent, FileKind::Directory, path)?;
        if self.find_entry(parent, &name)?.is_some() {
            return Err(FsError::AlreadyExists(path.into()));
        }
        let new_id = self.image.alloc_inode(FileKind::Directory)?;
        self.add_dir_entry(parent, &name, new_id)?;
        Ok(())
    }

    /// Writes `data` to `path`, replacing any existing contents.
    ///
    /// Returns the number of bytes written.
    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<usize> {
        let id = self.resolve_path(path)?;
        self.assert_kind(id, FileKind::File, path)?;
        self.truncate(id);
        if data.len() as u64 > self.image.sb.max_file_size() {
            return Err(FsError::FileTooLarge);
        }
        let bs = self.image.sb.block_size as usize;
        let total = data.len();
        let mut written = 0;
        let mut block_index = 0usize;
        while written < total {
            let chunk = (total - written).min(bs);
            let block = self.alloc_logical_block(id, block_index)?;
            let dst = &mut self.image.blocks[block as usize];
            dst[..chunk].copy_from_slice(&data[written..written + chunk]);
            for byte in dst.iter_mut().skip(chunk) {
                *byte = 0;
            }
            written += chunk;
            block_index += 1;
        }
        self.image.inodes[id as usize].size = total as u64;
        Ok(total)
    }

    /// Returns the full contents of `path`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let id = self.resolve_path(path)?;
        self.assert_kind(id, FileKind::File, path)?;
        let inode = &self.image.inodes[id as usize];
        let bs = self.image.sb.block_size as usize;
        let mut out = Vec::with_capacity(inode.size as usize);
        let mut remaining = inode.size as usize;
        let mut block_index = 0usize;
        while remaining > 0 {
            let blk = self.logical_block(inode, block_index).ok_or_else(|| {
                FsError::Corrupt(format!("missing block {block_index} for inode {id}"))
            })?;
            let chunk = remaining.min(bs);
            out.extend_from_slice(&self.image.blocks[blk as usize][..chunk]);
            remaining -= chunk;
            block_index += 1;
        }
        Ok(out)
    }

    /// Removes a regular file. Directories must be empty and removed via
    /// [`Fs::rmdir`].
    pub fn unlink(&mut self, path: &str) -> Result<()> {
        let (parent_parts, name) = match split_parent(path)? {
            Some(t) => t,
            None => return Err(FsError::IsADirectory("/".into())),
        };
        let parent = self.resolve_components(&parent_parts)?;
        let entry = self
            .find_entry(parent, &name)?
            .ok_or_else(|| FsError::NotFound(path.into()))?;
        let id = entry.inode;
        if self.image.inodes[id as usize].kind == FileKind::Directory {
            return Err(FsError::IsADirectory(path.into()));
        }
        self.remove_dir_entry(parent, &name)?;
        self.image.free_inode(id);
        Ok(())
    }

    /// Removes an empty directory.
    pub fn rmdir(&mut self, path: &str) -> Result<()> {
        let (parent_parts, name) = match split_parent(path)? {
            Some(t) => t,
            None => return Err(FsError::IsADirectory("/".into())),
        };
        let parent = self.resolve_components(&parent_parts)?;
        let entry = self
            .find_entry(parent, &name)?
            .ok_or_else(|| FsError::NotFound(path.into()))?;
        let id = entry.inode;
        if self.image.inodes[id as usize].kind != FileKind::Directory {
            return Err(FsError::NotADirectory(path.into()));
        }
        if !self.readdir_id(id)?.is_empty() {
            return Err(FsError::AlreadyExists(format!("{path} not empty")));
        }
        self.remove_dir_entry(parent, &name)?;
        self.image.free_inode(id);
        Ok(())
    }

    /// Lists entries in the directory at `path`.
    pub fn readdir(&self, path: &str) -> Result<Vec<Entry>> {
        let id = self.resolve_path(path)?;
        self.assert_kind(id, FileKind::Directory, path)?;
        self.readdir_id(id)
    }

    /// Returns metadata for the entry at `path`.
    pub fn stat(&self, path: &str) -> Result<Stat> {
        let id = self.resolve_path(path)?;
        let inode = &self.image.inodes[id as usize];
        let blocks = self.image.gather_pointers(id).len() as u32;
        Ok(Stat {
            inode: inode.id,
            kind: inode.kind,
            size: inode.size,
            link_count: inode.link_count,
            blocks,
        })
    }

    /// Walks every inode and reports inconsistencies — bitmap mismatches,
    /// double-allocated blocks, orphaned inodes, and so on.
    pub fn fsck(&self) -> FsckReport {
        let mut report = FsckReport::default();
        let mut seen = vec![0u8; self.image.sb.num_blocks as usize];
        let mut reachable = vec![false; INODE_COUNT];
        self.walk_reachable(0, &mut reachable, &mut report);

        for (id, inode) in self.image.inodes.iter().enumerate() {
            if !inode.allocated {
                continue;
            }
            if !reachable[id] && id != 0 {
                report
                    .issues
                    .push(format!("inode {id} allocated but unreachable"));
            }
            for blk in self.image.gather_pointers(id as u32) {
                if blk as usize >= seen.len() {
                    report
                        .issues
                        .push(format!("inode {id} points outside data area"));
                    continue;
                }
                if seen[blk as usize] > 0 {
                    report
                        .issues
                        .push(format!("block {blk} referenced multiple times"));
                }
                seen[blk as usize] = seen[blk as usize].saturating_add(1);
                if !self.image.bitmap[blk as usize] {
                    report
                        .issues
                        .push(format!("block {blk} used but bitmap marks free"));
                }
            }
        }
        for (i, used) in self.image.bitmap.iter().enumerate() {
            if *used && seen[i] == 0 {
                report
                    .issues
                    .push(format!("block {i} marked used but unreferenced"));
            }
        }
        report
    }

    /// Serialises the image to `writer` in the gridfs binary format.
    pub fn dump_image<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_all(MAGIC)?;
        writer.write_all(&self.image.sb.block_size.to_le_bytes())?;
        writer.write_all(&self.image.sb.num_blocks.to_le_bytes())?;
        writer.write_all(&self.image.sb.num_inodes.to_le_bytes())?;
        writer.write_all(&self.image.sb.root_inode.to_le_bytes())?;

        for inode in &self.image.inodes {
            writer.write_all(&inode.id.to_le_bytes())?;
            writer.write_all(&[inode.kind.to_byte(), inode.allocated as u8])?;
            writer.write_all(&inode.link_count.to_le_bytes())?;
            writer.write_all(&inode.size.to_le_bytes())?;
            for ptr in inode.direct.iter() {
                writer.write_all(&ptr.to_le_bytes())?;
            }
            writer.write_all(&inode.indirect.to_le_bytes())?;
        }
        // Bitmap packed into bytes, LSB-first.
        let mut byte = 0u8;
        let mut count = 0u8;
        for used in &self.image.bitmap {
            if *used {
                byte |= 1 << count;
            }
            count += 1;
            if count == 8 {
                writer.write_all(&[byte])?;
                byte = 0;
                count = 0;
            }
        }
        if count > 0 {
            writer.write_all(&[byte])?;
        }
        for block in &self.image.blocks {
            writer.write_all(block)?;
        }
        Ok(())
    }

    /// Loads an image previously produced by [`Fs::dump_image`].
    pub fn load_image<R: Read>(reader: &mut R) -> Result<Self> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(FsError::Corrupt("bad magic".into()));
        }
        let block_size = read_u32(reader)?;
        let num_blocks = read_u32(reader)?;
        let num_inodes = read_u32(reader)?;
        let root_inode = read_u32(reader)?;
        if num_inodes as usize != INODE_COUNT {
            return Err(FsError::Corrupt("inode count mismatch".into()));
        }
        let sb = Superblock {
            block_size,
            num_blocks,
            num_inodes,
            root_inode,
        };
        let mut inodes = Vec::with_capacity(INODE_COUNT);
        for _ in 0..INODE_COUNT {
            let id = read_u32(reader)?;
            let mut flags = [0u8; 2];
            reader.read_exact(&mut flags)?;
            let kind = FileKind::from_byte(flags[0])
                .ok_or_else(|| FsError::Corrupt("bad file kind".into()))?;
            let allocated = flags[1] != 0;
            let link_count = read_u16(reader)?;
            let size = read_u64(reader)?;
            let mut direct = [NULL_BLOCK; DIRECT_POINTERS];
            for entry in direct.iter_mut() {
                *entry = read_u32(reader)?;
            }
            let indirect = read_u32(reader)?;
            inodes.push(Inode {
                id,
                kind,
                size,
                direct,
                indirect,
                link_count,
                allocated,
            });
        }
        let bitmap_bytes = num_blocks.div_ceil(8) as usize;
        let mut packed = vec![0u8; bitmap_bytes];
        reader.read_exact(&mut packed)?;
        let mut bitmap = vec![false; num_blocks as usize];
        for (i, slot) in bitmap.iter_mut().enumerate() {
            *slot = (packed[i / 8] >> (i % 8)) & 1 != 0;
        }
        let mut blocks = Vec::with_capacity(num_blocks as usize);
        for _ in 0..num_blocks {
            let mut buf = vec![0u8; block_size as usize];
            reader.read_exact(&mut buf)?;
            blocks.push(buf);
        }
        Ok(Fs {
            image: Image {
                sb,
                inodes,
                bitmap,
                blocks,
            },
        })
    }

    // -- internal helpers --------------------------------------------------

    fn assert_kind(&self, id: InodeId, want: FileKind, path: &str) -> Result<()> {
        let kind = self.image.inodes[id as usize].kind;
        if kind == want {
            Ok(())
        } else if want == FileKind::Directory {
            Err(FsError::NotADirectory(path.into()))
        } else {
            Err(FsError::IsADirectory(path.into()))
        }
    }

    fn resolve_path(&self, path: &str) -> Result<InodeId> {
        let parts = split_path(path)?;
        self.resolve_components(&parts)
    }

    fn resolve_components(&self, parts: &[String]) -> Result<InodeId> {
        let mut current: InodeId = 0;
        for part in parts {
            self.assert_kind(current, FileKind::Directory, part)?;
            match self.find_entry(current, part)? {
                Some(e) => current = e.inode,
                None => return Err(FsError::NotFound(part.clone())),
            }
        }
        Ok(current)
    }

    fn readdir_id(&self, dir: InodeId) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for entry in self.iter_dir_entries(dir)? {
            if !entry.used {
                continue;
            }
            let kind = self.image.inodes[entry.inode as usize].kind;
            out.push(Entry {
                name: entry.name,
                inode: entry.inode,
                kind,
            });
        }
        Ok(out)
    }

    fn iter_dir_entries(&self, dir: InodeId) -> Result<Vec<DirEntry>> {
        let inode = &self.image.inodes[dir as usize];
        let bs = self.image.sb.block_size as usize;
        let entries_per_block = bs / DIR_ENTRY_SIZE;
        let mut out = Vec::new();
        let total_blocks = (inode.size as usize).div_ceil(bs);
        for block_idx in 0..total_blocks {
            let blk = self
                .logical_block(inode, block_idx)
                .ok_or_else(|| FsError::Corrupt("missing directory block".into()))?;
            let data = &self.image.blocks[blk as usize];
            for slot in 0..entries_per_block {
                let off = slot * DIR_ENTRY_SIZE;
                let entry = DirEntry::decode(&data[off..off + DIR_ENTRY_SIZE])?;
                out.push(entry);
            }
        }
        Ok(out)
    }

    fn find_entry(&self, dir: InodeId, name: &str) -> Result<Option<DirEntry>> {
        for entry in self.iter_dir_entries(dir)? {
            if entry.used && entry.name == name {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    fn add_dir_entry(&mut self, dir: InodeId, name: &str, target: InodeId) -> Result<()> {
        let bs = self.image.sb.block_size as usize;
        let entries_per_block = bs / DIR_ENTRY_SIZE;
        let dir_size = self.image.inodes[dir as usize].size as usize;
        let total_blocks = dir_size.div_ceil(bs);

        // First try to reuse an unused slot.
        for block_idx in 0..total_blocks {
            let inode = &self.image.inodes[dir as usize];
            let blk = self
                .logical_block(inode, block_idx)
                .ok_or_else(|| FsError::Corrupt("missing directory block".into()))?;
            for slot in 0..entries_per_block {
                let off = slot * DIR_ENTRY_SIZE;
                let entry =
                    DirEntry::decode(&self.image.blocks[blk as usize][off..off + DIR_ENTRY_SIZE])?;
                if !entry.used {
                    let de = DirEntry {
                        inode: target,
                        used: true,
                        name_len: name.len() as u8,
                        name: name.to_string(),
                    };
                    de.encode(&mut self.image.blocks[blk as usize][off..off + DIR_ENTRY_SIZE]);
                    return Ok(());
                }
            }
        }
        // Grow the directory by one block.
        let new_block = self.alloc_logical_block(dir, total_blocks)?;
        let de = DirEntry {
            inode: target,
            used: true,
            name_len: name.len() as u8,
            name: name.to_string(),
        };
        de.encode(&mut self.image.blocks[new_block as usize][0..DIR_ENTRY_SIZE]);
        // Mark the rest of the block as empty entries.
        for slot in 1..entries_per_block {
            let off = slot * DIR_ENTRY_SIZE;
            DirEntry::empty()
                .encode(&mut self.image.blocks[new_block as usize][off..off + DIR_ENTRY_SIZE]);
        }
        self.image.inodes[dir as usize].size += bs as u64;
        Ok(())
    }

    fn remove_dir_entry(&mut self, dir: InodeId, name: &str) -> Result<()> {
        let bs = self.image.sb.block_size as usize;
        let entries_per_block = bs / DIR_ENTRY_SIZE;
        let total_blocks = (self.image.inodes[dir as usize].size as usize).div_ceil(bs);
        for block_idx in 0..total_blocks {
            let inode = &self.image.inodes[dir as usize];
            let blk = self
                .logical_block(inode, block_idx)
                .ok_or_else(|| FsError::Corrupt("missing directory block".into()))?;
            for slot in 0..entries_per_block {
                let off = slot * DIR_ENTRY_SIZE;
                let entry =
                    DirEntry::decode(&self.image.blocks[blk as usize][off..off + DIR_ENTRY_SIZE])?;
                if entry.used && entry.name == name {
                    DirEntry::empty()
                        .encode(&mut self.image.blocks[blk as usize][off..off + DIR_ENTRY_SIZE]);
                    return Ok(());
                }
            }
        }
        Err(FsError::NotFound(name.into()))
    }

    fn truncate(&mut self, id: InodeId) {
        let pointers = self.image.gather_pointers(id);
        for blk in pointers {
            self.image.free_block(blk);
        }
        let inode = &mut self.image.inodes[id as usize];
        inode.direct = [NULL_BLOCK; DIRECT_POINTERS];
        inode.indirect = NULL_BLOCK;
        inode.size = 0;
    }

    fn logical_block(&self, inode: &Inode, block_index: usize) -> Option<u32> {
        if block_index < DIRECT_POINTERS {
            let b = inode.direct[block_index];
            if b == NULL_BLOCK {
                None
            } else {
                Some(b)
            }
        } else {
            if inode.indirect == NULL_BLOCK {
                return None;
            }
            let pointers_per_block = self.image.sb.block_size as usize / 4;
            let off_idx = block_index - DIRECT_POINTERS;
            if off_idx >= pointers_per_block {
                return None;
            }
            let off = off_idx * 4;
            let blk = &self.image.blocks[inode.indirect as usize];
            let ptr = u32::from_le_bytes(blk[off..off + 4].try_into().unwrap());
            if ptr == NULL_BLOCK {
                None
            } else {
                Some(ptr)
            }
        }
    }

    fn alloc_logical_block(&mut self, id: InodeId, block_index: usize) -> Result<u32> {
        if let Some(b) = self.logical_block(&self.image.inodes[id as usize], block_index) {
            return Ok(b);
        }
        if block_index < DIRECT_POINTERS {
            let new = self.image.alloc_block()?;
            self.image.inodes[id as usize].direct[block_index] = new;
            Ok(new)
        } else {
            let pointers_per_block = self.image.sb.block_size as usize / 4;
            let off_idx = block_index - DIRECT_POINTERS;
            if off_idx >= pointers_per_block {
                return Err(FsError::FileTooLarge);
            }
            if self.image.inodes[id as usize].indirect == NULL_BLOCK {
                let new = self.image.alloc_block()?;
                // Initialize all pointers in the indirect block to NULL.
                let blk = &mut self.image.blocks[new as usize];
                for slot in 0..pointers_per_block {
                    let off = slot * 4;
                    blk[off..off + 4].copy_from_slice(&NULL_BLOCK.to_le_bytes());
                }
                self.image.inodes[id as usize].indirect = new;
            }
            let new = self.image.alloc_block()?;
            let indirect = self.image.inodes[id as usize].indirect as usize;
            let off = off_idx * 4;
            self.image.blocks[indirect][off..off + 4].copy_from_slice(&new.to_le_bytes());
            Ok(new)
        }
    }

    fn walk_reachable(&self, dir: InodeId, seen: &mut [bool], report: &mut FsckReport) {
        if (dir as usize) >= seen.len() || seen[dir as usize] {
            return;
        }
        seen[dir as usize] = true;
        let inode = &self.image.inodes[dir as usize];
        if inode.kind != FileKind::Directory {
            return;
        }
        let entries = match self.iter_dir_entries(dir) {
            Ok(e) => e,
            Err(e) => {
                report
                    .issues
                    .push(format!("failed to read directory inode {dir}: {e}"));
                return;
            }
        };
        for entry in entries {
            if !entry.used {
                continue;
            }
            if (entry.inode as usize) >= seen.len() {
                report.issues.push(format!(
                    "directory entry points to invalid inode {}",
                    entry.inode
                ));
                continue;
            }
            let target = &self.image.inodes[entry.inode as usize];
            if !target.allocated {
                report.issues.push(format!(
                    "entry '{}' points at unallocated inode",
                    entry.name
                ));
            }
            self.walk_reachable(entry.inode, seen, report);
        }
    }
}

fn read_u16<R: Read>(reader: &mut R) -> Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(reader: &mut R) -> Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_read_write_roundtrip() {
        let mut fs = Fs::new(128, 32).unwrap();
        fs.create("/a.txt").unwrap();
        fs.write("/a.txt", b"hello").unwrap();
        assert_eq!(fs.read("/a.txt").unwrap(), b"hello");
    }

    #[test]
    fn mkdir_and_readdir() {
        let mut fs = Fs::new(128, 32).unwrap();
        fs.mkdir("/dir").unwrap();
        fs.create("/dir/file").unwrap();
        let entries = fs.readdir("/dir").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file");
    }
}
