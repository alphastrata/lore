// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT

//! Timing for a filter shaped like a long-lived game project's `.loreignore`: deep
//! anchored directory rules, extension rules, and re-inclusions over the names they hold.
//!
//! This is the arbiter the depth-gate fold experiment asked for: synthetic benches hold
//! shallow rules that stay eligible at every depth, so a min_depth gate prunes nothing and
//! loses to indirection. A real project holds most of its rules deep, where mid-depth
//! folds shed most of the run.
//!
//! Run with:
//!     `cargo test -p lore-revision --release --test real_filter_bench -- --ignored --nocapture --test-threads=1`

use lore_revision::filter::Filter;
use lore_revision::filter::FilterMode;
use lore_revision::util::path::RelativePathBuf;

/// Rules shaped like a game project's ignore list: engines, build trees, intermediates,
/// third-party bins, editor extensions, and the source re-inclusions that reopen them.
fn rules() -> Vec<&'static str> {
    let mut rules: Vec<&'static str> = Vec::new();
    for engine_dir in [
        "Binaries", "Build", "Config", "Content/Collections", "Content/Developer",
        "Content/Editor", "DerivedDataCache", "Intermediate", "Saved", "Scratch",
        "Documentation/Builds", "Engine/Content/Slate/Testing", "Engine/Plugins/Playgrounds",
        "Samples/Media", "Extras/Vehicles", "Features", "Templates",
    ] {
        rules.push(&*Box::leak(format!("/{engine_dir}").into_boxed_str()));
    }
    for plugin_tree in [
        "Plugins/Animation/ControlRig", "Plugins/AudioModulation", "Plugins/Media/OBS",
        "Plugins/MovieScene/Sequencer", "Plugins/VirtualProduction/LedWall",
        "Plugins/Physics/ChaosSolver", "Plugins/MeshEditing/EditorOnly",
    ] {
        for sub in ["Binaries", "Intermediate", "Saved", "DerivedDataCache"] {
            rules.push(&*Box::leak(format!("/{plugin_tree}/{sub}").into_boxed_str()));
        }
        rules.push(&*Box::leak(format!("/{plugin_tree}/Content/Collections").into_boxed_str()));
    }
    for deep in [
        "Source/ThirdParty/AMD/AGS/bin", "Source/ThirdParty/NVIDIA/NVaftermath/lib",
        "Source/ThirdParty/Intel/oneTBB/bin", "Source/Runtime/Launch/Private/Windows",
        "Source/Runtime/Renderer/Private/VT", "Shaders/Shared/Cache/Compiled",
    ] {
        rules.push(&*Box::leak(format!("/{deep}").into_boxed_str()));
    }
    for ext in [
        "obj", "bin", "pdb", "exe", "dll", "lib", "so", "dylib", "ipa", "apk", "sln", "suo",
        "csproj", "vcxproj", "objcache", "uasset.tmp", "log", "tlog", "resharper", "DS_Store",
    ] {
        rules.push(&*Box::leak(format!("*.{ext}").into_boxed_str()));
    }
    for mid in ["Game/Content/Developers", "Game/Content/__ExternalActors", "Maps/Collab"] {
        rules.push(&*Box::leak(format!("/{mid}").into_boxed_str()));
    }
    rules.push("!/Content/Vehicles/SharedMaterials");
    rules.push("!/Plugins/Animation/ControlRig/Content/Public");
    rules
}

fn build() -> Filter {
    let mut filter = Filter::default();
    for rule in rules() {
        if let Some(rest) = rule.strip_prefix('!') {
            filter.view.add_inclusion(rest).expect("view inclusion");
        } else {
            filter.view.add_exclusion(rule).expect("view exclusion");
        }
    }
    filter
}

fn deep_paths(count: usize) -> Vec<(RelativePathBuf, bool)> {
    let mut paths = Vec::new();
    let mut dirs = 0usize;
    let mut i = 0;
    while paths.len() < count {
        let branch = format!("branch{}", i % 12);
        for (tree, tail) in [
            (
                "Engine/Plugins/Animation/ControlRig",
                "Content/Public/RigUnits/Transforms/Nodes",
            ),
            (
                "Game/Content/Characters/Mannequin",
                "Materials/Textures/UE4Logos",
            ),
            ("Source/Runtime/Engine/Private", "Rendering/Composition/Lighting"),
            ("Game/Maps/Collab/DesertSector", "Landscape/Layers/Detail"),
            ("Game/Content/Developers/alice/WIP", "TestMaps/Variants"),
            ("Shaders/Shared/Cache/Compiled", "Vehicles/Wheels/Fallback"),
        ] {
            let mut path = RelativePathBuf::new();
            path.push(tree);
            path.push(&branch);
            path.push(tail);
            path.push(format!("node{}.txt", i % 97));
            let is_directory = dirs % 2 == 0;
            dirs += 1;
            paths.push((path, is_directory));
            if paths.len() >= count {
                break;
            }
        }
        i += 1;
    }
    paths
}

fn time(label: &str, calls: usize, paths: &[(RelativePathBuf, bool)], filter: &Filter) {
    let mut sink = 0u64;
    let start = std::time::Instant::now();
    let mut i = 0;
    while i < calls {
        let (path, is_directory) = &paths[i % paths.len()];
        if filter.excludes(path, *is_directory, FilterMode::Full) {
            sink += 1;
        }
        i += 1;
    }
    let per = start.elapsed() / calls as u32;
    println!("{label:<10} {calls} calls -> {per:?} per call (sink {sink})");
}

#[test]
#[ignore = "benchmark: run on demand with --ignored"]
fn a_game_project_filter_over_deep_paths() {
    let filter = build();
    let lines = filter.view.lines.len();
    println!("compiled lines {}", lines);
    let paths = deep_paths(600);
    time("whole path", 30000, &paths, &filter);
    time("repeats", 60000, &paths, &filter);
}
