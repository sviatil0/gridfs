//! End-to-end integration tests for the public [`Fs`] surface.

use gridfs::{FileKind, Fs, FsError};
use std::io::Cursor;

#[test]
fn create_read_write_delete_lifecycle() {
    let mut fs = Fs::new(128, 32).unwrap();
    fs.create("/foo.txt").unwrap();
    fs.write("/foo.txt", b"the quick brown fox").unwrap();
    assert_eq!(fs.read("/foo.txt").unwrap(), b"the quick brown fox");

    fs.unlink("/foo.txt").unwrap();
    assert!(matches!(
        fs.read("/foo.txt").unwrap_err(),
        FsError::NotFound(_)
    ));
}

#[test]
fn mkdir_and_readdir_lists_children() {
    let mut fs = Fs::new(256, 16).unwrap();
    fs.mkdir("/etc").unwrap();
    fs.mkdir("/etc/conf.d").unwrap();
    fs.create("/etc/hosts").unwrap();

    let mut entries: Vec<_> = fs.readdir("/etc").unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "conf.d");
    assert_eq!(entries[0].kind, FileKind::Directory);
    assert_eq!(entries[1].name, "hosts");
    assert_eq!(entries[1].kind, FileKind::File);
}

#[test]
fn fills_block_bitmap_until_full() {
    // Only 4 data blocks; the root directory will consume one.
    let mut fs = Fs::new(128, 4).unwrap();
    fs.create("/a").unwrap();
    let payload = vec![0xab; 128];
    fs.write("/a", &payload).unwrap();
    // Now try to write a payload that needs more blocks than remain.
    fs.create("/b").unwrap();
    let big = vec![0u8; 128 * 16];
    let err = fs.write("/b", &big).unwrap_err();
    assert!(matches!(err, FsError::OutOfSpace));
}

#[test]
fn unlink_frees_blocks() {
    let mut fs = Fs::new(128, 16).unwrap();
    fs.create("/x").unwrap();
    fs.write("/x", &vec![1u8; 256]).unwrap();
    let used_before = fs.fsck();
    assert!(used_before.is_clean());

    let stat_before = fs.stat("/x").unwrap();
    assert!(stat_before.blocks > 0);
    fs.unlink("/x").unwrap();

    // Create another file of the same size — it must succeed because the
    // blocks were freed.
    fs.create("/y").unwrap();
    fs.write("/y", &vec![2u8; 256]).unwrap();
    let after = fs.fsck();
    assert!(after.is_clean(), "fsck issues: {:?}", after.issues);
}

#[test]
fn save_and_load_roundtrip() {
    let mut fs = Fs::new(256, 16).unwrap();
    fs.mkdir("/d").unwrap();
    fs.create("/d/h").unwrap();
    fs.write("/d/h", b"persisted").unwrap();

    let mut buf = Vec::new();
    fs.dump_image(&mut buf).unwrap();
    let mut cursor = Cursor::new(buf);
    let restored = Fs::load_image(&mut cursor).unwrap();
    assert_eq!(restored.read("/d/h").unwrap(), b"persisted");
    let entries = restored.readdir("/d").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "h");
}

#[test]
fn fsck_reports_clean_image() {
    let mut fs = Fs::new(128, 8).unwrap();
    fs.mkdir("/a").unwrap();
    fs.create("/a/b").unwrap();
    fs.write("/a/b", b"abc").unwrap();
    assert!(fs.fsck().is_clean());
}

#[test]
fn rejects_invalid_paths() {
    let mut fs = Fs::new(128, 8).unwrap();
    assert!(matches!(
        fs.create("relative").unwrap_err(),
        FsError::InvalidPath(_)
    ));
    assert!(matches!(
        fs.create("/../escape").unwrap_err(),
        FsError::InvalidPath(_)
    ));
}

#[test]
fn cannot_unlink_directory() {
    let mut fs = Fs::new(128, 8).unwrap();
    fs.mkdir("/d").unwrap();
    let err = fs.unlink("/d").unwrap_err();
    assert!(matches!(err, FsError::IsADirectory(_)));
}

#[test]
fn writes_larger_than_direct_pointers_use_indirect() {
    // block_size = 128, direct pointers cover 12 * 128 = 1536 bytes.
    let mut fs = Fs::new(128, 64).unwrap();
    fs.create("/big").unwrap();
    let data = vec![0x55u8; 4096];
    fs.write("/big", &data).unwrap();
    assert_eq!(fs.read("/big").unwrap(), data);
    let stat = fs.stat("/big").unwrap();
    assert!(stat.blocks > 12);
    assert!(fs.fsck().is_clean());
}

#[test]
fn rewriting_a_file_reclaims_blocks() {
    let mut fs = Fs::new(128, 16).unwrap();
    fs.create("/f").unwrap();
    fs.write("/f", &vec![9u8; 512]).unwrap();
    // Overwrite with a smaller payload — old blocks should be freed.
    fs.write("/f", b"short").unwrap();
    assert_eq!(fs.read("/f").unwrap(), b"short");
    assert!(fs.fsck().is_clean());
}
