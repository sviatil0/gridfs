//! Criterion benchmarks for gridfs throughput.
//!
//! Run with `cargo bench`. The benchmarks measure write throughput, read
//! throughput, and directory traversal latency.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use gridfs::Fs;

fn bench_write(c: &mut Criterion) {
    let payload = vec![0xa5u8; 64 * 1024];
    let mut group = c.benchmark_group("write");
    group.throughput(Throughput::Bytes(payload.len() as u64));
    group.bench_function("64KiB", |b| {
        b.iter(|| {
            let mut fs = Fs::new(4096, 64).unwrap();
            fs.create("/f").unwrap();
            fs.write("/f", black_box(&payload)).unwrap();
        })
    });
    group.finish();
}

fn bench_read(c: &mut Criterion) {
    let payload = vec![0x42u8; 64 * 1024];
    let mut fs = Fs::new(4096, 64).unwrap();
    fs.create("/f").unwrap();
    fs.write("/f", &payload).unwrap();
    let mut group = c.benchmark_group("read");
    group.throughput(Throughput::Bytes(payload.len() as u64));
    group.bench_function("64KiB", |b| b.iter(|| black_box(fs.read("/f").unwrap())));
    group.finish();
}

fn bench_walk(c: &mut Criterion) {
    let mut fs = Fs::new(1024, 64).unwrap();
    fs.mkdir("/dir").unwrap();
    for i in 0..16 {
        fs.create(&format!("/dir/f{i}")).unwrap();
    }
    c.bench_function("readdir-16", |b| {
        b.iter(|| black_box(fs.readdir("/dir").unwrap()))
    });
}

criterion_group!(benches, bench_write, bench_read, bench_walk);
criterion_main!(benches);
