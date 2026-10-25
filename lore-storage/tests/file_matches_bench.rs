// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT

//! Timing for `file_matches` over files already in the store, which is the compare a
//! status, stage, or commit scan runs for every file it visits.
//!
//! Each timed call asks a fresh [`ContentHashes`], so the whole-content compare runs its
//! full read each time, the way it does on the walk the comparison serves. The store is
//! warm: fixtures are written once, then the pages they live on are as cached as a real
//! scan's would be.
//!
//! Run with:
//!     `cargo test -p lore-storage --release --test file_matches_bench -- --ignored --nocapture`
//!
//! `LORE_BENCH_FILES` lists the file count and defaults to 2000. `LORE_BENCH_SIZE_KIB` lists
//! each file's size and defaults to 8, which lands under [`FRAGMENT_SIZE_MINIMUM`], where the
//! whole-content compare answers unconditionally.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lore_storage::content::ContentSource;
use lore_storage::local::immutable_store::ImmutableStoreSettings;
use lore_storage::local::immutable_store::LocalImmutableStore;
use lore_storage::Partition;
use lore_storage::write::file_matches;
use lore_storage::write::write_from_file;
use lore_storage::write::ContentHashes;
use lore_storage::options::WriteOptions;
use lore_storage::write_tracker::WriteContext;
use lore_storage::Context;
use lore_storage::FileMatch;
use lore_storage::ImmutableStore;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: run on demand with --ignored"]
async fn file_matches_throughput() {
    let files: usize = std::env::var("LORE_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let size_kib: usize = std::env::var("LORE_BENCH_SIZE_KIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let size = size_kib * 1024;
    let rounds: usize = 3;

    let root = std::env::temp_dir().join(format!(
        "lore-file-matches-bench-{}-{}",
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

    let mut addresses = Vec::with_capacity(files);
    let mut paths = Vec::with_capacity(files);
    for file in 0..files {
        let path = root.join(format!("bench-{file}"));
        let mut content = vec![0u8; size];
        content[0..8].copy_from_slice(&(file as u64).to_le_bytes());
        std::fs::write(&path, &content).expect("write fixture");
        let source = ContentSource::file(&path);
        let written = write_from_file(
            store.clone(),
            partition,
            &source,
            Context::default(),
            WriteOptions::default().no_remote_write(),
            None,
            WriteContext::none(),
        )
        .await
        .expect("store fixture");
        addresses.push(written.address);
        paths.push(path);
    }

    // One untiming pass proves the compare answers for what was stored.
    for (path, previous) in paths.iter().zip(&addresses) {
        let source = ContentSource::file(path);
        let established = ContentHashes::default();
        let matched = file_matches(
            store.clone(),
            partition,
            *previous,
            None,
            None,
            true,
            &source,
            &established,
        )
        .await
        .expect("compare fixture");
        assert!(
            matches!(matched, FileMatch::Match),
            "a compare against its own store must match"
        );
    }

    let mut total = std::time::Duration::ZERO;
    for _ in 0..rounds {
        let start = Instant::now();
        for (path, previous) in paths.iter().zip(&addresses) {
            let source = ContentSource::file(path);
            let established = ContentHashes::default();
            let matched = file_matches(
                store.clone(),
                partition,
                *previous,
                None,
                None,
                true,
                &source,
                &established,
            )
            .await
            .expect("compare fixture");
            assert!(matches!(matched, FileMatch::Match));
        }
        total += start.elapsed();
    }

    let calls = files * rounds;
    println!(
        "file_matches  {calls} calls over {files} files of {size_kib} KiB -> {:?} per call {:?} total",
        total / calls as u32,
        total
    );

    std::fs::remove_dir_all(&root).ok();
    let _ = PathBuf::new();
}
