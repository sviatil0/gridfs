//! Inode definitions for gridfs.
//!
//! Each inode tracks one file or directory and points at the data blocks
//! holding its contents. The pointer scheme is a small homage to classic Unix
//! filesystems: every inode has [`DIRECT_POINTERS`] direct block pointers and
//! a single indirect pointer. The indirect block is itself a data block that
//! stores an array of further block indices.
//!
//! For directories, the data blocks contain a serialized list of
//! [`DirEntry`](crate::block::DirEntry) records.

/// Number of direct block pointers stored inline in each inode.
pub const DIRECT_POINTERS: usize = 12;

/// Sentinel value used to mark an unused block pointer.
pub const NULL_BLOCK: u32 = u32::MAX;

/// Discriminates between regular files and directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// An ordinary, byte-oriented file.
    File,
    /// A container that holds [`DirEntry`](crate::block::DirEntry) records.
    Directory,
}

impl FileKind {
    pub(crate) fn to_byte(self) -> u8 {
        match self {
            FileKind::File => 1,
            FileKind::Directory => 2,
        }
    }

    pub(crate) fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(FileKind::File),
            2 => Some(FileKind::Directory),
            _ => None,
        }
    }
}

/// Index of an inode within the inode table.
pub type InodeId = u32;

/// Metadata + block pointers for a single file or directory.
#[derive(Debug, Clone)]
pub struct Inode {
    /// Self-referential inode id; matches the position in the inode table.
    pub id: InodeId,
    /// Whether this inode describes a file or a directory.
    pub kind: FileKind,
    /// Logical size in bytes.
    pub size: u64,
    /// Direct block pointers; [`NULL_BLOCK`] when unused.
    pub direct: [u32; DIRECT_POINTERS],
    /// Optional single-indirect block; [`NULL_BLOCK`] when unused.
    pub indirect: u32,
    /// Reference count - 1 for a freshly created inode.
    pub link_count: u16,
    /// `true` while this inode is in active use.
    pub allocated: bool,
}

impl Inode {
    /// Constructs a freshly allocated inode of the given kind.
    pub fn new(id: InodeId, kind: FileKind) -> Self {
        Self {
            id,
            kind,
            size: 0,
            direct: [NULL_BLOCK; DIRECT_POINTERS],
            indirect: NULL_BLOCK,
            link_count: 1,
            allocated: true,
        }
    }

    /// Returns a placeholder inode used to fill empty table slots.
    pub fn empty(id: InodeId) -> Self {
        let mut node = Self::new(id, FileKind::File);
        node.allocated = false;
        node.link_count = 0;
        node
    }
}
