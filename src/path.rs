//! Path parsing and validation utilities.
//!
//! gridfs uses a Unix-style hierarchical path syntax. Every path must be
//! absolute, i.e. begin with `/`. Empty components (created by trailing
//! slashes or `//`) are ignored. The reserved components `.` and `..` are
//! rejected to keep the implementation small and predictable.

use crate::error::{FsError, Result};

/// Splits an absolute path into its non-empty components.
///
/// # Examples
///
/// ```
/// use gridfs::path::split_path;
/// assert_eq!(split_path("/a/b/c").unwrap(), vec!["a", "b", "c"]);
/// assert_eq!(split_path("/").unwrap(), Vec::<String>::new());
/// ```
pub fn split_path(path: &str) -> Result<Vec<String>> {
    if !path.starts_with('/') {
        return Err(FsError::InvalidPath(path.to_string()));
    }
    let mut out = Vec::new();
    for part in path.split('/') {
        if part.is_empty() {
            continue;
        }
        if part == "." || part == ".." {
            return Err(FsError::InvalidPath(path.to_string()));
        }
        if part.len() > 255 {
            return Err(FsError::InvalidPath(path.to_string()));
        }
        out.push(part.to_string());
    }
    Ok(out)
}

/// Splits a path into its parent path and final component (the basename).
///
/// Returns `None` for the root path.
pub fn split_parent(path: &str) -> Result<Option<(Vec<String>, String)>> {
    let mut parts = split_path(path)?;
    match parts.pop() {
        Some(name) => Ok(Some((parts, name))),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_root() {
        assert!(split_path("/").unwrap().is_empty());
    }

    #[test]
    fn rejects_relative() {
        assert!(split_path("a/b").is_err());
    }

    #[test]
    fn rejects_dotdot() {
        assert!(split_path("/a/../b").is_err());
    }

    #[test]
    fn handles_double_slash() {
        assert_eq!(split_path("//a//b//").unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn parent_root() {
        assert!(split_parent("/").unwrap().is_none());
    }

    #[test]
    fn parent_simple() {
        let (parent, name) = split_parent("/a/b").unwrap().unwrap();
        assert_eq!(parent, vec!["a"]);
        assert_eq!(name, "b");
    }
}
