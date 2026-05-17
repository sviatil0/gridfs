//! Error types for the gridfs filesystem.
//!
//! All fallible operations return [`Result<T>`](crate::Result), which is an
//! alias for `std::result::Result<T, FsError>`. The error enum is constructed
//! via [`thiserror`] so each variant carries a clear, user-facing message.

use std::io;
use thiserror::Error;

/// The error type returned by all filesystem operations.
#[derive(Debug, Error)]
pub enum FsError {
    /// The supplied path was empty, malformed, or did not start with `/`.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// A path component could not be resolved.
    #[error("no such file or directory: {0}")]
    NotFound(String),

    /// An entry already exists at the destination.
    #[error("file already exists: {0}")]
    AlreadyExists(String),

    /// Caller attempted a file operation on a directory or vice versa.
    #[error("not a directory: {0}")]
    NotADirectory(String),

    /// Caller attempted a directory operation on a regular file.
    #[error("is a directory: {0}")]
    IsADirectory(String),

    /// No free inodes remain in the inode table.
    #[error("inode table is full")]
    OutOfInodes,

    /// No free data blocks remain in the bitmap.
    #[error("no space left on device")]
    OutOfSpace,

    /// File is too large to be represented by the inode pointer scheme.
    #[error("file exceeds maximum addressable size")]
    FileTooLarge,

    /// Image bytes did not match the expected on-disk format.
    #[error("corrupt image: {0}")]
    Corrupt(String),

    /// An underlying I/O error occurred while persisting or loading an image.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

/// Convenience alias for fallible filesystem operations.
pub type Result<T> = std::result::Result<T, FsError>;
