// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT

//! Timing for `write_resolved_from_file` over content already held, which is the publish a
//! resolved write of a small file pays on every scan and stage that maps working content.
//!
//! Run with:
//!     `cargo test -p lore-storage --release --test resolved_publish_bench -- --ignored --nocapture`
//! `LORE_BENCH_FILES` and `LORE_BENCH_SIZE_KIB` shape the fixture, defaulting to 1000 x 8 KiB.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lore_storage::content::ContentSource;
use lore_storage::local::immutable_store::ImmutableStoreSettings;
use lore_storage::local::immutable_store::LocalImmutableStore;
use lore_storage::options::WriteOptions;
use lore_storage::write::write_resolved_from_file;
use lore_storage::write_tracker::WriteContext;
use lore_storage::hash;
use lore_storage::Context;
use lore_storage::ImmutableStore;
use lore_storage::MutableStoreSettings;
use lore_storage::Partition;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: run on demand with --ignored"]
async fn resolved_publish_throughput() {
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
        "lore-resolved-publish-bench-{}-{}",
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
    let mutable = lore_storage::local::mutable_store::create(
        None::<&str>,
        MutableStoreSettings::default(),
        store.clone(),
    )
    .await
    .expect("create bench mutable store");
    let partition = Partition::default();
    let flags = WriteOptions::default().no_remote_write();

    let mut paths = Vec::with_capacity(files);
    for file in 0..files {
        let path = root.join(format!("bench-{file}"));
        let mut content = vec![0u8; size];
        content[0..8].copy_from_slice(&(file as u64).to_le_bytes());
        std::fs::write(&path, &content).expect("write fixture");
        paths.push(path);
    }

    // One untiming pass warms the store pages and proves the publishes answer.
    for (file, path) in paths.iter().enumerate() {
        let key = hash::hash_slice(format!("bench-key-{file}").as_bytes());
        write_resolved_from_file(
            store.clone(),
            mutable.clone(),
            partition,
            key,
            Context::default(),
            path,
            flags.clone(),
            None,
            WriteContext::none(),
        )
        .await
        .expect("publish fixture");
    }

    let mut total = std::time::Duration::ZERO;
    for _ in 0..rounds {
        let start = Instant::now();
        for (file, path) in paths.iter().enumerate() {
            let key = hash::hash_slice(format!("bench-key-{file}").as_bytes());
            let source = ContentSource::file(path);
            let _ = source;
            write_resolved_from_file(
                store.clone(),
                mutable.clone(),
                partition,
                key,
                Context::default(),
                path,
                flags.clone(),
                None,
                WriteContext::none(),
            )
            .await
            .expect("publish fixture");
        }
        total += start.elapsed();
    }
    let calls = files * rounds;
    println!(
        "resolved      {calls} calls over {files} files of {size_kib} KiB -> {:?} per call {:?} total",
        total / calls as u32,
        total
    );

    std::fs::remove_dir_all(&root).ok();
    let _ = PathBuf::new();
}
