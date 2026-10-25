// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT

//! Timing for `hash_file` over files already stored, which is the hash a stage or status
//! scan computes for every small file it visits.
//!
//! Run with:
//!     `cargo test -p lore-storage --release --test hash_file_bench -- --ignored --nocapture`
//! `LORE_BENCH_FILES` and `LORE_BENCH_SIZE_KIB` shape the fixture, defaulting to 1000 x 8 KiB.

use std::sync::Arc;
use std::time::Instant;

use lore_storage::content::ContentSource;
use lore_storage::local::immutable_store::ImmutableStoreSettings;
use lore_storage::local::immutable_store::LocalImmutableStore;
use lore_storage::write::hash_file;
use lore_storage::Partition;
use lore_storage::Context;
use lore_storage::ImmutableStore;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: run on demand with --ignored"]
async fn hash_small_file_throughput() {
    let files: usize = std::env::var("LORE_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let size_kib: usize = std::env::var("LORE_BENCH_SIZE_KIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let size = size_kib * 1024;
    let rounds: usize = 3;

    let root = std::env::temp_dir().join(format!(
        "lore-hash-file-bench-{}-{}",
        std::process::id(),
        size_kib
    ));
    std::fs::create_dir_all(&root).expect("create bench root");

    let store: Arc<dyn ImmutableStore> = LocalImmutableStore::new(
        Some(root.clone()),
        ImmutableStoreSettings::default(),
    )
    .await
    .expect("create bench store");
    let partition = Partition::default();

    let mut paths = Vec::with_capacity(files);
    for file in 0..files {
        let path = root.join(format!("bench-{file}"));
        let mut content = vec![0u8; size];
        content[0..8].copy_from_slice(&(file as u64).to_le_bytes());
        std::fs::write(&path, &content).expect("write fixture");
        paths.push(path);
    }

    // One untiming pass warms the store pages and proves the hashes answer.
    for path in &paths {
        let source = ContentSource::file(path);
        let hash = hash_file(store.clone(), partition, &source, None)
            .await
            .expect("hash fixture");
        assert!(!hash.is_zero(), "a hashed fixture hashes to something");
    }

    let mut total = std::time::Duration::ZERO;
    for _ in 0..rounds {
        let start = Instant::now();
        for path in &paths {
            let source = ContentSource::file(path);
            hash_file(store.clone(), partition, &source, None)
                .await
                .expect("hash fixture");
        }
        total += start.elapsed();
    }
    let calls = files * rounds;
    println!(
        "hash_file     {calls} calls over {files} files of {size_kib} KiB -> {:?} per call {:?} total",
        total / calls as u32,
        total
    );

    std::fs::remove_dir_all(&root).ok();
}
