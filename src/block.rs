//! Block-layer primitives: the superblock, inode table, block bitmap, and
//! the raw data area.
//!
//! Layout (logical):
//!
//! ```text
//! +--------------+-----------------+--------------+-------------+
//! |  Superblock  |   Inode Table   | Block Bitmap | Data Blocks |
//! +--------------+-----------------+--------------+-------------+
//! ```
//!
//! Each data block is a fixed-size `Vec<u8>`. Inodes own block indices
//! into the data area. Free blocks are tracked by a `Vec<bool>` bitmap. The
//! image serializer in [`crate::Fs`] flattens these structures to a single
//! byte stream.

use crate::error::{FsError, Result};
use crate::inode::{FileKind, Inode, InodeId, DIRECT_POINTERS, NULL_BLOCK};

/// Magic bytes used to identify a gridfs image on disk.
pub const MAGIC: &[u8; 8] = b"GRIDFS01";

/// Number of inodes allocated up front. Fixed for simplicity.
pub const INODE_COUNT: usize = 256;

/// Maximum number of bytes per filename inside a directory entry.
pub const MAX_NAME_LEN: usize = 60;

/// On-disk size of a serialized [`DirEntry`].
pub const DIR_ENTRY_SIZE: usize = 4 + 1 + 1 + 2 + MAX_NAME_LEN;

/// A single record inside a directory's data blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Inode this entry points at.
    pub inode: InodeId,
    /// `true` while this slot is in use.
    pub used: bool,
    /// Length of the name in bytes.
    pub name_len: u8,
    /// UTF-8 name, padded with zero bytes to [`MAX_NAME_LEN`].
    pub name: String,
}

impl DirEntry {
    /// Returns a placeholder, marked unused.
    pub fn empty() -> Self {
        Self {
            inode: 0,
            used: false,
            name_len: 0,
            name: String::new(),
        }
    }

    /// Serializes the entry into `buf`, which must be at least
    /// [`DIR_ENTRY_SIZE`] bytes long.
    pub fn encode(&self, buf: &mut [u8]) {
        debug_assert!(buf.len() >= DIR_ENTRY_SIZE);
        buf[0..4].copy_from_slice(&self.inode.to_le_bytes());
        buf[4] = if self.used { 1 } else { 0 };
        buf[5] = self.name_len;
        buf[6..8].copy_from_slice(&0u16.to_le_bytes());
        for byte in buf.iter_mut().take(DIR_ENTRY_SIZE).skip(8) {
            *byte = 0;
        }
        let bytes = self.name.as_bytes();
        let n = bytes.len().min(MAX_NAME_LEN);
        buf[8..8 + n].copy_from_slice(&bytes[..n]);
    }

    /// Parses a [`DirEntry`] from `buf`.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < DIR_ENTRY_SIZE {
            return Err(FsError::Corrupt("directory entry truncated".into()));
        }
        let inode = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let used = buf[4] != 0;
        let name_len = buf[5];
        if name_len as usize > MAX_NAME_LEN {
            return Err(FsError::Corrupt("directory name overflows slot".into()));
        }
        let name_bytes = &buf[8..8 + name_len as usize];
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| FsError::Corrupt("directory name not utf-8".into()))?
            .to_string();
        Ok(Self {
            inode,
            used,
            name_len,
            name,
        })
    }
}

/// Top-level metadata describing image geometry.
#[derive(Debug, Clone)]
pub struct Superblock {
    /// Bytes per data block.
    pub block_size: u32,
    /// Total number of data blocks.
    pub num_blocks: u32,
    /// Inodes provisioned in the image (always [`INODE_COUNT`] today).
    pub num_inodes: u32,
    /// Root inode id (always 0).
    pub root_inode: u32,
}

impl Superblock {
    /// Returns the maximum addressable file size given the current geometry.
    pub fn max_file_size(&self) -> u64 {
        let direct = DIRECT_POINTERS as u64 * self.block_size as u64;
        let pointers_per_block = self.block_size as u64 / 4;
        let indirect = pointers_per_block * self.block_size as u64;
        direct + indirect
    }
}

/// In-memory image — superblock, inodes, free-block bitmap, and data area.
#[derive(Debug)]
pub struct Image {
    /// Geometry of the image.
    pub sb: Superblock,
    /// Fixed-size table of all inodes.
    pub inodes: Vec<Inode>,
    /// One flag per data block; `true` means allocated.
    pub bitmap: Vec<bool>,
    /// Raw data area, one `Vec<u8>` per data block.
    pub blocks: Vec<Vec<u8>>,
}

impl Image {
    /// Creates a fresh, freshly formatted image of the given geometry.
    ///
    /// Allocates the root directory inode and zeroes the data area.
    pub fn new(block_size: usize, num_blocks: usize) -> Result<Self> {
        if block_size < DIR_ENTRY_SIZE {
            return Err(FsError::Corrupt(format!(
                "block size must be at least {DIR_ENTRY_SIZE} bytes"
            )));
        }
        if num_blocks == 0 {
            return Err(FsError::Corrupt("num_blocks must be > 0".into()));
        }
        let sb = Superblock {
            block_size: block_size as u32,
            num_blocks: num_blocks as u32,
            num_inodes: INODE_COUNT as u32,
            root_inode: 0,
        };
        let inodes = (0..INODE_COUNT as u32).map(Inode::empty).collect();
        let bitmap = vec![false; num_blocks];
        let blocks = vec![vec![0u8; block_size]; num_blocks];
        let mut img = Self {
            sb,
            inodes,
            bitmap,
            blocks,
        };
        // Initialise the root directory.
        let mut root = Inode::new(0, FileKind::Directory);
        root.link_count = 1;
        img.inodes[0] = root;
        Ok(img)
    }

    /// Returns the first free data block, marks it as allocated, and zeros it.
    pub fn alloc_block(&mut self) -> Result<u32> {
        for (i, used) in self.bitmap.iter_mut().enumerate() {
            if !*used {
                *used = true;
                for byte in self.blocks[i].iter_mut() {
                    *byte = 0;
                }
                return Ok(i as u32);
            }
        }
        Err(FsError::OutOfSpace)
    }

    /// Releases a previously allocated data block.
    pub fn free_block(&mut self, idx: u32) {
        if (idx as usize) < self.bitmap.len() {
            self.bitmap[idx as usize] = false;
        }
    }

    /// Allocates a free inode of the given kind.
    pub fn alloc_inode(&mut self, kind: FileKind) -> Result<InodeId> {
        for inode in self.inodes.iter_mut() {
            if !inode.allocated {
                *inode = Inode::new(inode.id, kind);
                return Ok(inode.id);
            }
        }
        Err(FsError::OutOfInodes)
    }

    /// Frees the inode at `id` and releases all blocks it referenced.
    pub fn free_inode(&mut self, id: InodeId) {
        let pointers = self.gather_pointers(id);
        for blk in pointers {
            self.free_block(blk);
        }
        let inode = &mut self.inodes[id as usize];
        *inode = Inode::empty(id);
    }

    /// Returns every data block referenced by an inode, including the indirect
    /// block itself.
    pub fn gather_pointers(&self, id: InodeId) -> Vec<u32> {
        let mut out = Vec::new();
        let inode = &self.inodes[id as usize];
        if !inode.allocated {
            return out;
        }
        for &b in inode.direct.iter() {
            if b != NULL_BLOCK {
                out.push(b);
            }
        }
        if inode.indirect != NULL_BLOCK {
            out.push(inode.indirect);
            let pointers_per_block = self.sb.block_size as usize / 4;
            let blk = &self.blocks[inode.indirect as usize];
            for i in 0..pointers_per_block {
                let off = i * 4;
                let ptr = u32::from_le_bytes(blk[off..off + 4].try_into().unwrap());
                if ptr != NULL_BLOCK {
                    out.push(ptr);
                }
            }
        }
        out
    }
}
