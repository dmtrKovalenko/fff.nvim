//! Stress tests for the bigram overlay layer.
//!
//! These tests create a real git repository with many files, build the bigram
//! index, then perform loops of creates / edits / deletes and verify that grep
//! always returns correct results through the overlay. Finally, a git commit +
//! rescan cycle verifies the index is rebuilt cleanly.
//!
//! # Known bugs exposed by these tests
//!
//! 1. **Overflow files invisible to grep** — `grep_search` only iterates
//!    base-file bits from the bigram candidate bitset. Overflow files
//!    (index >= base_count) are never included. `BigramOverlay::query_added`
//!    exists but is dead code — never called from the grep path.
//!    See: grep.rs lines ~1787-1855.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

use fff_search::file_picker::{FFFMode, FilePicker, FuzzySearchOptions};
use fff_search::grep::{GrepMode, GrepSearchOptions, parse_grep_query};
use fff_search::{FilePickerOptions, PaginationArgs, QueryParser, SharedFrecency, SharedPicker};

/// Stress test: 50 base files, 3 rounds of edits + deletes. New files
/// are tracked but NOT verified via grep (see Group 3 for that bug).
#[test]
fn bigram_overlay_coherence_stress_base_edits_and_deletes() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    let initial_files = seed_files(base, 50);
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    // Sanity: all 50 tokens findable.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        for (_, token) in &initial_files {
            assert!(
                grep_count(picker, token) >= 1,
                "Initial token {token} should be findable"
            );
        }
    }

    let mut live_tokens: Vec<(String, String)> = initial_files.clone();
    let mut dead_tokens: Vec<String> = Vec::new();

    // Sleep so mtime advances past the scan snapshot.
    std::thread::sleep(Duration::from_millis(1100));

    for round in 0..3 {
        // ── DELETE: remove first 5 live base files ──
        let delete_count = 5.min(live_tokens.len());
        let to_delete: Vec<(String, String)> = live_tokens.drain(..delete_count).collect();
        for (name, token) in &to_delete {
            let path = base.join(name);
            fs::remove_file(&path).unwrap();
            {
                let mut guard = shared_picker.write().unwrap();
                let picker = guard.as_mut().unwrap();
                assert!(
                    picker.remove_file_by_path(&path),
                    "round {round}: remove({name}) should succeed"
                );
            }
            dead_tokens.push(token.clone());
        }

        // ── EDIT: modify next 5 live base files ──
        let edit_count = 5.min(live_tokens.len());
        for i in 0..edit_count {
            let (ref name, ref mut old_token) = live_tokens[i];
            let new_token = format!("EDITED_R{round}_{i:04}");
            write_file_with_token(base, name, &new_token);
            {
                let mut guard = shared_picker.write().unwrap();
                let picker = guard.as_mut().unwrap();
                assert!(
                    picker.on_create_or_modify(base.join(name)).is_some(),
                    "round {round}: modify({name}) should succeed"
                );
            }
            dead_tokens.push(old_token.clone());
            *old_token = new_token;
        }

        // ── VERIFY: all live base tokens findable, all dead tokens gone ──
        {
            let guard = shared_picker.read().unwrap();
            let picker = guard.as_ref().unwrap();

            for (name, token) in &live_tokens {
                let count = grep_count(picker, token);
                assert!(
                    count >= 1,
                    "round {round}: live token {token} ({name}) should be findable, got {count}"
                );
            }

            for token in &dead_tokens {
                assert_eq!(
                    grep_count(picker, token),
                    0,
                    "round {round}: dead token {token} should NOT be findable"
                );
            }
        }
    }

    stop_picker(&shared_picker);
}

/// Simulate a long editing session: 10 rounds of edits to 20 base files.
/// Only the latest content should be searchable.
#[test]
fn bigram_overlay_coherence_long_session_incremental_edits() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    let file_count = 20;
    let edits_per_file = 10;
    seed_files(base, file_count);
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    let mut latest_tokens: Vec<String> = (0..file_count)
        .map(|i| format!("SEED_TOKEN_{i:04}"))
        .collect();

    for edit_round in 0..edits_per_file {
        for file_idx in 0..file_count {
            let name = format!("file_{file_idx:04}.rs");
            let new_token = format!("LONG_EDIT_F{file_idx:04}_R{edit_round:04}");
            write_file_with_token(base, &name, &new_token);
            {
                let mut guard = shared_picker.write().unwrap();
                let picker = guard.as_mut().unwrap();
                picker.on_create_or_modify(base.join(&name));
            }
            latest_tokens[file_idx] = new_token;
        }
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();

        for (file_idx, token) in latest_tokens.iter().enumerate() {
            assert!(
                grep_count(picker, token) >= 1,
                "file_{file_idx:04}: latest token {token} should be findable"
            );
        }

        // Spot-check old tokens are gone.
        for file_idx in [0, 5, 10, 15, 19] {
            if file_idx >= file_count {
                continue;
            }
            let seed = format!("SEED_TOKEN_{file_idx:04}");
            assert_eq!(
                grep_count(picker, &seed),
                0,
                "file_{file_idx:04}: seed token should not be findable"
            );
            let mid = format!("LONG_EDIT_F{file_idx:04}_R0003");
            assert_eq!(
                grep_count(picker, &mid),
                0,
                "file_{file_idx:04}: intermediate token should not be findable"
            );
        }
    }

    stop_picker(&shared_picker);
}

/// Resurrect tombstoned base files: delete then re-create with new content.
#[test]
fn bigram_overlay_coherence_resurrect_tombstoned_file() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    seed_files(base, 10);
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    let target = "file_0003.rs";
    let target_path = base.join(target);

    // Delete.
    fs::remove_file(&target_path).unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.remove_file_by_path(&target_path);
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert_eq!(grep_count(picker, "SEED_TOKEN_0003"), 0);
    }

    // Re-create with new content.
    write_file_with_token(base, target, "RESURRECTED_TOKEN");
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        assert!(picker.on_create_or_modify(&target_path).is_some());
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert!(
            grep_count(picker, "RESURRECTED_TOKEN") >= 1,
            "resurrected token should be findable"
        );
        assert_eq!(
            grep_count(picker, "SEED_TOKEN_0003"),
            0,
            "old token should not be findable after resurrect"
        );
    }

    stop_picker(&shared_picker);
}

/// Prove the overlay is doing real work for base file modifications.
/// Uses files with diverse content so the bigram index is discriminating.
#[test]
fn bigram_overlay_coherence_proves_contribution_for_modified_base() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    // Create files with DIVERSE content so bigram index builds effective
    // discriminating columns (avoids ubiquitous/sparse column pruning).
    fs::write(
        base.join("alpha.rs"),
        "fn alpha() { let x = calculate_velocity(params); }\n",
    )
    .unwrap();
    fs::write(
        base.join("beta.rs"),
        "fn beta() { database_query(sql_string); }\n",
    )
    .unwrap();
    fs::write(
        base.join("gamma.rs"),
        "fn gamma() { render_template(html_buffer); }\n",
    )
    .unwrap();
    fs::write(
        base.join("delta.rs"),
        "fn delta() { network_request(endpoint_url); }\n",
    )
    .unwrap();
    fs::write(
        base.join("epsilon.rs"),
        "fn epsilon() { parse_json_payload(raw_bytes); }\n",
    )
    .unwrap();

    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Replace beta.rs with content containing a unique token that shares
    // NO bigrams with its original content "database_query(sql_string)".
    let unique = "XYZZY_PLUGH_WIZARDRY";
    fs::write(
        base.join("beta.rs"),
        format!("fn beta() {{ println!(\"{unique}\"); }}\n"),
    )
    .unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.on_create_or_modify(base.join("beta.rs"));
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();

        let with_overlay = grep_count(picker, unique);
        assert_eq!(with_overlay, 1, "overlay should find the new token");

        let without_overlay = grep_without_overlay_count(picker, unique);
        assert_eq!(
            without_overlay, 0,
            "without overlay, bigram should exclude beta.rs (stale bigrams)"
        );
    }

    stop_picker(&shared_picker);
}

/// Rapidly create-then-delete the same base file path 10 times.
#[test]
fn bigram_overlay_coherence_rapid_create_delete_same_base_path() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    // volatile.rs is a BASE file (exists before index build).
    fs::write(base.join("anchor.txt"), "anchor\n").unwrap();
    fs::write(base.join("volatile.rs"), "initial volatile content\n").unwrap();
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    let volatile_path = base.join("volatile.rs");

    for cycle in 0..10 {
        let token = format!("VOLATILE_CYCLE_{cycle:03}");

        // Overwrite.
        write_file_with_token(base, "volatile.rs", &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(&volatile_path);
        }

        {
            let guard = shared_picker.read().unwrap();
            let picker = guard.as_ref().unwrap();
            assert!(
                grep_count(picker, &token) >= 1,
                "cycle {cycle}: {token} should be findable"
            );
            if cycle > 0 {
                let prev = format!("VOLATILE_CYCLE_{:03}", cycle - 1);
                assert_eq!(
                    grep_count(picker, &prev),
                    0,
                    "cycle {cycle}: previous token should be gone"
                );
            }
        }

        // Delete (tombstone).
        fs::remove_file(&volatile_path).unwrap();
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.remove_file_by_path(&volatile_path);
        }

        {
            let guard = shared_picker.read().unwrap();
            let picker = guard.as_ref().unwrap();
            assert_eq!(
                grep_count(picker, &token),
                0,
                "cycle {cycle}: {token} should be gone after delete"
            );
        }

        // Re-create for next cycle.
        write_file_with_token(base, "volatile.rs", &format!("placeholder_{cycle}"));
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(&volatile_path);
        }
    }

    stop_picker(&shared_picker);
}

/// New overflow files should be searchable via grep.
///
/// To trigger the bug, we need an effective bigram index (diverse file
/// content so prefiltering is actually active). With identical-template
/// files, ALL bigrams are either ubiquitous (in every file) or unique
/// (in only one file), so the index retains no columns and prefiltering
/// is always bypassed — masking the bug.
///
/// Here we seed 20 files with deliberately varied content so bigram
/// columns have moderate cardinality (retained by the index), making
/// the prefilter discriminating.
#[test]
fn bigram_overlay_coherence_overflow_files_searchable_via_grep() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    // Varied content — each file has unique text with different bigram profiles.
    let words = [
        "calculate_velocity",
        "database_migration",
        "render_template",
        "network_request",
        "parse_json_payload",
        "compress_archive",
        "validate_schema",
        "transform_matrix",
        "schedule_pipeline",
        "authenticate_user",
        "encrypt_message",
        "decode_packet",
        "allocate_buffer",
        "serialize_config",
        "optimize_query",
        "register_handler",
        "dispatch_event",
        "synchronize_state",
        "initialize_module",
        "aggregate_metrics",
    ];
    for (i, word) in words.iter().enumerate() {
        let name = format!("module_{i:02}.rs");
        let content = format!(
            "pub fn {word}() -> Result<(), Error> {{\n    \
             let data = process_{word}_input();\n    \
             log::info!(\"executing {word}\");\n    \
             Ok(())\n}}\n"
        );
        fs::write(base.join(&name), content).unwrap();
    }

    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    // Verify bigram index was built and is effective.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert!(picker.bigram_index().is_some());
        assert!(picker.bigram_overlay().is_some());

        // Sanity: a query with content from one file finds exactly that file.
        assert!(
            grep_count(picker, "calculate_velocity") >= 1,
            "sanity: known content should be findable"
        );
    }

    std::thread::sleep(Duration::from_millis(1100));

    // Add a new overflow file whose content has bigrams shared with
    // existing files (e.g., "calculate" shares bigrams with module_00).
    let new_path = base.join("overflow_new.rs");
    fs::write(
        &new_path,
        "pub fn recalculate_velocity_delta() {\n    \
         let v = calculate_velocity();\n    \
         println!(\"delta: {}\", v);\n}\n",
    )
    .unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        assert!(picker.on_create_or_modify(&new_path).is_some());
    }

    // Overflow file is tracked.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert_eq!(picker.get_overflow_files().len(), 1);
    }

    // BUG: grep_search's bigram candidate bitset only covers base files
    // (indices 0..base_count). Overflow files at index >= base_count are
    // never set as candidates. BigramOverlay::query_added() exists but
    // is dead code — never called from the grep path.
    //
    // Search for content unique to the overflow file.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert!(
            grep_count(picker, "recalculate_velocity_delta") >= 1,
            "BUG: overflow file content should be findable via grep but \
             bigram prefiltering skips overflow files"
        );
    }

    stop_picker(&shared_picker);
}

/// Mixed tombstones and overflow: delete base files AND add new ones.
/// Tombstone assertions pass; overflow grep assertions expose the bug.
#[test]
fn bigram_overlay_coherence_mixed_tombstones_and_overflow() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    seed_files(base, 30);
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Delete first 15 base files.
    for i in 0..15 {
        let name = format!("file_{i:04}.rs");
        let path = base.join(&name);
        fs::remove_file(&path).unwrap();
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.remove_file_by_path(&path);
    }

    // Add 15 new overflow files.
    let mut new_tokens = Vec::new();
    for i in 0..15 {
        let name = format!("replacement_{i:04}.rs");
        let token = format!("REPLACEMENT_{i:04}");
        write_file_with_token(base, &name, &token);
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.on_create_or_modify(base.join(&name));
        new_tokens.push(token);
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();

        // Tombstones work: deleted tokens are gone.
        for i in 0..15 {
            let token = format!("SEED_TOKEN_{i:04}");
            assert_eq!(
                grep_count(picker, &token),
                0,
                "deleted file_{i:04} token should not be findable"
            );
        }

        // Surviving base files still findable.
        for i in 15..30 {
            let token = format!("SEED_TOKEN_{i:04}");
            assert!(
                grep_count(picker, &token) >= 1,
                "surviving file_{i:04} token should be findable"
            );
        }

        // BUG: Overflow files not findable via grep.
        for token in &new_tokens {
            assert!(
                grep_count(picker, token) >= 1,
                "BUG: overflow token {token} should be findable but bigram skips overflow"
            );
        }
    }

    stop_picker(&shared_picker);
}

#[test]
fn bigram_overlay_coherence_full_stress_loop_with_overflow() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    let initial_files = seed_files(base, 50);
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    let mut live_base: Vec<(String, String)> = initial_files;
    let mut live_overflow: Vec<(String, String)> = Vec::new();
    let mut dead_tokens: Vec<String> = Vec::new();

    std::thread::sleep(Duration::from_millis(1100));

    for round in 0..3 {
        // ── DELETE 5 base files ──
        let to_delete: Vec<_> = live_base.drain(..5).collect();
        for (name, token) in &to_delete {
            let path = base.join(name);
            fs::remove_file(&path).unwrap();
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            assert!(picker.remove_file_by_path(&path));
            dead_tokens.push(token.clone());
        }

        // ── EDIT 5 base files ──
        for i in 0..5.min(live_base.len()) {
            let (ref name, ref mut old_token) = live_base[i];
            let new_token = format!("EDITED_R{round}_{i:04}");
            write_file_with_token(base, name, &new_token);
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(name));
            dead_tokens.push(old_token.clone());
            *old_token = new_token;
        }

        // ── CREATE 5 overflow files ──
        for i in 0..5 {
            let name = format!("new_r{round}_{i:04}.rs");
            let token = format!("NEWFILE_R{round}_{i:04}");
            write_file_with_token(base, &name, &token);
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(&name));
            live_overflow.push((name, token));
        }

        // ── VERIFY base files ──
        {
            let guard = shared_picker.read().unwrap();
            let picker = guard.as_ref().unwrap();

            for (name, token) in &live_base {
                assert!(
                    grep_count(picker, token) >= 1,
                    "round {round}: base token {token} ({name}) should be findable"
                );
            }

            for token in &dead_tokens {
                assert_eq!(
                    grep_count(picker, token),
                    0,
                    "round {round}: dead token {token} should NOT be findable"
                );
            }

            // BUG: overflow files not findable via grep.
            for (name, token) in &live_overflow {
                assert!(
                    grep_count(picker, token) >= 1,
                    "round {round}: BUG overflow token {token} ({name}) not findable via grep"
                );
            }
        }
    }

    stop_picker(&shared_picker);
}

/// Overflow files can be edited and deleted through the picker.
/// Verifies the overlay tracks overflow changes even though grep may not
/// search them (that's the separate bug in Group 2).
#[test]
fn bigram_overlay_coherence_overflow_file_edit_and_delete() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    fs::write(base.join("base.txt"), "base file content\n").unwrap();
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Add overflow files and track their paths.
    let mut overflow_files: Vec<(PathBuf, String)> = Vec::new();
    for i in 0..10 {
        let name = format!("overflow_{i}.rs");
        let token = format!("OVERFLOW_ORIG_{i}");
        let path = base.join(&name);
        write_file_with_token(base, &name, &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(&path);
        }
        overflow_files.push((path, token));
    }

    // Verify overflow files are tracked in the picker.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert_eq!(
            picker.get_overflow_files().len(),
            10,
            "should have 10 overflow files"
        );
    }

    // Edit all overflow files.
    let mut edited_tokens = Vec::new();
    for (i, (path, _)) in overflow_files.iter().enumerate() {
        let name = path.file_name().unwrap().to_str().unwrap();
        let new_token = format!("OVERFLOW_EDITED_{i}");
        write_file_with_token(base, name, &new_token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(path);
        }
        edited_tokens.push(new_token);
    }

    // Delete first 5 overflow files.
    for i in 0..5 {
        let (ref path, _) = overflow_files[i];
        fs::remove_file(path).unwrap();
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.remove_file_by_path(path);
        }
    }

    // Verify 5 overflow remain.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert_eq!(
            picker.get_overflow_files().len(),
            5,
            "should have 5 overflow files after deleting 5"
        );
    }

    stop_picker(&shared_picker);
}

// ═══════════════════════════════════════════════════════════════════════
// Group 4: Rescan after git commit. Tests that `trigger_rescan` picks
// up committed changes and that the file list is refreshed.
// ═══════════════════════════════════════════════════════════════════════

/// After editing base files, committing, and rescanning, the new content
/// should be searchable.
///
/// NOTE: `trigger_rescan` replaces `sync_data` which drops the bigram
/// index (it lives inside `FileSync`). After rescan, grep falls back to
/// full search (no bigram prefiltering) which is correct but slower.
#[test]
fn bigram_overlay_coherence_rescan_after_git_commit() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    seed_files(base, 30);
    git_init_and_commit(base);

    let (shared_picker, shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Edit 10 base files and add 5 new files.
    let mut edited_tokens = Vec::new();
    for i in 0..10 {
        let name = format!("file_{i:04}.rs");
        let token = format!("PRE_COMMIT_EDIT_{i:04}");
        write_file_with_token(base, &name, &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(&name));
        }
        edited_tokens.push(token);
    }

    let mut new_tokens = Vec::new();
    for i in 0..5 {
        let name = format!("committed_new_{i:04}.rs");
        let token = format!("COMMITTED_NEW_{i:04}");
        write_file_with_token(base, &name, &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(&name));
        }
        new_tokens.push(token);
    }

    // Phase 2: Commit and rescan.
    git_add_and_commit(base, "batch edit");

    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker
            .trigger_rescan(&shared_frecency)
            .expect("trigger_rescan should succeed");
    }

    // After trigger_rescan, sync_data is replaced (and bigram_index dropped
    // with it). Wait for the synchronous scan to finish.
    wait_for_scan(&shared_picker);

    // Verify the file list is refreshed: all 35 files (30 + 5 new) should
    // be present as base files (not overflow, since they're committed).
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        assert_eq!(
            picker.get_files().len(),
            35,
            "post-rescan: should have 35 files (30 original + 5 new)"
        );
        // After rescan, new files are in the base (no overflow).
        assert_eq!(
            picker.get_overflow_files().len(),
            0,
            "post-rescan: should have 0 overflow files"
        );
    }

    // Post-rescan grep verification.
    // After rescan + bigram rebuild, edited tokens should be in the base
    // index and findable without the overlay.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();

        for token in &edited_tokens {
            let with = grep_count(picker, token);
            assert!(
                with >= 1,
                "post-rescan: edited token {token} should be findable"
            );

            // The content is now in the base index, so it should be
            // findable even without the overlay.
            let without = grep_without_overlay_count(picker, token);
            assert!(
                without >= 1,
                "post-rescan: {token} should be in base index (without overlay: {without})"
            );
        }

        for token in &new_tokens {
            let with = grep_count(picker, token);
            assert!(
                with >= 1,
                "post-rescan: new token {token} should be findable"
            );
        }
    }

    stop_picker(&shared_picker);
}

// ═══════════════════════════════════════════════════════════════════════
// Group 5: Full lifecycle — seed, overlay edits, commit, rescan, more
// overlay edits. Tests the complete workflow.
// ═══════════════════════════════════════════════════════════════════════

/// seed → index → overlay edits → commit → rescan → more overlay edits
#[test]
fn bigram_overlay_coherence_full_lifecycle_seed_edit_commit_rescan_edit() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    let initial_count = 40;
    seed_files(base, initial_count);
    git_init_and_commit(base);

    let (shared_picker, shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // ── Phase 1: Pre-commit overlay edits (base files only) ──
    let mut phase1_tokens = Vec::new();
    for i in 0..10 {
        let name = format!("file_{i:04}.rs");
        let token = format!("PHASE1_EDIT_{i:04}");
        write_file_with_token(base, &name, &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(&name));
        }
        phase1_tokens.push(token);
    }

    // Delete some files.
    let mut phase1_dead = Vec::new();
    for i in 35..40 {
        let name = format!("file_{i:04}.rs");
        let path = base.join(&name);
        fs::remove_file(&path).unwrap();
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.remove_file_by_path(&path);
        }
        phase1_dead.push(format!("SEED_TOKEN_{i:04}"));
    }

    // Verify phase 1 (base file operations only).
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        for token in &phase1_tokens {
            assert!(
                grep_count(picker, token) >= 1,
                "phase1: {token} should be findable"
            );
        }
        for token in &phase1_dead {
            assert_eq!(
                grep_count(picker, token),
                0,
                "phase1: {token} should be gone"
            );
        }
    }

    // ── Phase 2: Commit and rescan ──
    git_add_and_commit(base, "phase 1 changes");

    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker
            .trigger_rescan(&shared_frecency)
            .expect("rescan should succeed");
    }

    // After rescan, bigram is dropped with old FileSync. Grep falls back
    // to full search, which is correct.
    wait_for_scan(&shared_picker);

    // Phase1 tokens should still be findable (now in base index).
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        for token in &phase1_tokens {
            assert!(
                grep_count(picker, token) >= 1,
                "post-rescan: {token} should still be findable"
            );
        }
        for token in &phase1_dead {
            assert_eq!(
                grep_count(picker, token),
                0,
                "post-rescan: {token} should not be findable"
            );
        }
    }

    std::thread::sleep(Duration::from_millis(1100));

    // ── Phase 3: More overlay edits after rescan ──
    let mut phase3_tokens = Vec::new();
    for i in 0..5 {
        let name = format!("file_{i:04}.rs");
        let token = format!("PHASE3_EDIT_{i:04}");
        write_file_with_token(base, &name, &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(&name));
        }
        phase3_tokens.push(token);
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();

        for token in &phase3_tokens {
            assert!(
                grep_count(picker, token) >= 1,
                "phase3: {token} should be findable"
            );
        }

        // Phase1 tokens for files 0-4 were overwritten by phase3.
        for i in 0..5 {
            let old = format!("PHASE1_EDIT_{i:04}");
            assert_eq!(
                grep_count(picker, &old),
                0,
                "phase3: overwritten {old} should not be findable"
            );
        }

        // Phase1 tokens for files 5-9 should still be there (in base).
        for i in 5..10 {
            let token = format!("PHASE1_EDIT_{i:04}");
            assert!(
                grep_count(picker, &token) >= 1,
                "phase3: {token} should still be findable"
            );
        }
    }

    stop_picker(&shared_picker);
}

// ═══════════════════════════════════════════════════════════════════════
// Group 6: Nested directory operations.
// ═══════════════════════════════════════════════════════════════════════

/// Files in nested directories should be editable via overlay.
#[test]
fn bigram_overlay_coherence_nested_directory_edits() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    let dirs = ["src", "src/core", "src/utils", "tests", "tests/integration"];
    for dir in &dirs {
        fs::create_dir_all(base.join(dir)).unwrap();
    }

    let mut initial_files = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        for j in 0..5 {
            let name = format!("{dir}/mod_{j}.rs");
            let token = format!("DIR{i}_FILE{j}_TOKEN");
            write_file_with_token(base, &name, &token);
            initial_files.push((name, token));
        }
    }

    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Verify all initial tokens findable.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        for (name, token) in &initial_files {
            assert!(
                grep_count(picker, token) >= 1,
                "initial token {token} ({name}) should be findable"
            );
        }
    }

    // Edit one file per directory (base file modifications).
    let mut edited = Vec::new();
    for dir in &dirs {
        let name = format!("{dir}/mod_0.rs");
        let token = format!("NESTED_EDIT_{}", dir.replace('/', "_"));
        write_file_with_token(base, &name, &token);
        {
            let mut guard = shared_picker.write().unwrap();
            let picker = guard.as_mut().unwrap();
            picker.on_create_or_modify(base.join(&name));
        }
        edited.push(token);
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        for token in &edited {
            assert!(
                grep_count(picker, token) >= 1,
                "nested edit token {token} should be findable"
            );
        }
    }

    stop_picker(&shared_picker);
}

// ═══════════════════════════════════════════════════════════════════════
// Group 7: Fuzzy file search. Verifies that fuzzy filename matching
// works for base files, edited files, overflow files, and after rescan.
// ═══════════════════════════════════════════════════════════════════════

/// Helper: run a fuzzy search and return matched file paths.
fn fuzzy_search_paths(picker: &FilePicker, query: &str) -> Vec<String> {
    let parser = QueryParser::default();
    let parsed = parser.parse(query);
    let result = FilePicker::fuzzy_search(
        picker.get_files(),
        &parsed,
        None,
        FuzzySearchOptions {
            max_threads: 1,
            pagination: PaginationArgs {
                offset: 0,
                limit: 200,
            },
            ..Default::default()
        },
    );
    result
        .items
        .iter()
        .map(|f| f.path_str().to_string())
        .collect()
}

/// Fuzzy search finds base files, overflow files, and respects deletions.
#[test]
fn bigram_overlay_coherence_fuzzy_search_base_overflow_and_deleted() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    // Create files with distinctive names for fuzzy matching.
    fs::write(base.join("controller_auth.rs"), "auth controller\n").unwrap();
    fs::write(base.join("controller_user.rs"), "user controller\n").unwrap();
    fs::write(base.join("model_invoice.rs"), "invoice model\n").unwrap();
    fs::write(base.join("service_payment.rs"), "payment service\n").unwrap();
    fs::write(base.join("helper_crypto.rs"), "crypto helper\n").unwrap();

    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    // Fuzzy search for "controller" — should find both controller files.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let results = fuzzy_search_paths(picker, "controller");
        assert!(
            results.len() >= 2,
            "fuzzy 'controller' should match at least 2 files, got {results:?}"
        );
        assert!(
            results.iter().any(|p| p.contains("controller_auth")),
            "should find controller_auth.rs"
        );
        assert!(
            results.iter().any(|p| p.contains("controller_user")),
            "should find controller_user.rs"
        );
    }

    std::thread::sleep(Duration::from_millis(1100));

    // Delete controller_user.rs (tombstone).
    let user_path = base.join("controller_user.rs");
    fs::remove_file(&user_path).unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.remove_file_by_path(&user_path);
    }

    // Fuzzy search should no longer return the deleted file.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let results = fuzzy_search_paths(picker, "controller");
        assert!(
            results.iter().any(|p| p.contains("controller_auth")),
            "should still find controller_auth.rs"
        );
        assert!(
            !results.iter().any(|p| p.contains("controller_user")),
            "deleted controller_user.rs should not appear in fuzzy results"
        );
    }

    // Add a new overflow file.
    fs::write(base.join("controller_admin.rs"), "admin controller\n").unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.on_create_or_modify(base.join("controller_admin.rs"));
    }

    // Fuzzy search should find the new overflow file.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let results = fuzzy_search_paths(picker, "controller");
        assert!(
            results.iter().any(|p| p.contains("controller_admin")),
            "overflow file controller_admin.rs should appear in fuzzy results, got {results:?}"
        );
        assert!(
            results.iter().any(|p| p.contains("controller_auth")),
            "base file controller_auth.rs should still be in fuzzy results"
        );
    }

    stop_picker(&shared_picker);
}

/// Fuzzy search works after rescan: base files + committed new files all
/// appear, and the deleted files are gone.
#[test]
fn bigram_overlay_coherence_fuzzy_search_after_rescan() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    fs::write(base.join("router_api.rs"), "api router\n").unwrap();
    fs::write(base.join("router_web.rs"), "web router\n").unwrap();
    fs::write(base.join("middleware_cors.rs"), "cors middleware\n").unwrap();

    git_init_and_commit(base);

    let (shared_picker, shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Add a new file and delete an existing one.
    fs::write(base.join("router_grpc.rs"), "grpc router\n").unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.on_create_or_modify(base.join("router_grpc.rs"));
    }

    let web_path = base.join("router_web.rs");
    fs::remove_file(&web_path).unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.remove_file_by_path(&web_path);
    }

    // Commit and rescan.
    git_add_and_commit(base, "add grpc, remove web");

    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker
            .trigger_rescan(&shared_frecency)
            .expect("rescan should succeed");
    }
    wait_for_scan(&shared_picker);

    // After rescan, fuzzy search should reflect the committed state.
    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let results = fuzzy_search_paths(picker, "router");
        assert!(
            results.iter().any(|p| p.contains("router_api")),
            "router_api.rs should be in results"
        );
        assert!(
            results.iter().any(|p| p.contains("router_grpc")),
            "committed router_grpc.rs should be in results, got {results:?}"
        );
        assert!(
            !results.iter().any(|p| p.contains("router_web")),
            "deleted router_web.rs should not be in results"
        );
    }

    stop_picker(&shared_picker);
}

/// Combined: fuzzy search + grep on the same overlay state. Exercises
/// both search paths after a mix of edits, creates, and deletes.
#[test]
fn bigram_overlay_coherence_fuzzy_and_grep_combined() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    seed_files(base, 30);
    git_init_and_commit(base);

    let (shared_picker, _shared_frecency) = make_picker(base);
    wait_for_bigram(&shared_picker);

    std::thread::sleep(Duration::from_millis(1100));

    // Edit a base file.
    write_file_with_token(base, "file_0005.rs", "COMBINED_EDIT_TOKEN");
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.on_create_or_modify(base.join("file_0005.rs"));
    }

    // Add an overflow file with a distinctive name.
    fs::write(
        base.join("unique_overflow_widget.rs"),
        "OVERFLOW_WIDGET_CONTENT\n",
    )
    .unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.on_create_or_modify(base.join("unique_overflow_widget.rs"));
    }

    // Delete a base file.
    let del_path = base.join("file_0010.rs");
    fs::remove_file(&del_path).unwrap();
    {
        let mut guard = shared_picker.write().unwrap();
        let picker = guard.as_mut().unwrap();
        picker.remove_file_by_path(&del_path);
    }

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();

        // Grep: edited content findable.
        assert!(
            grep_count(picker, "COMBINED_EDIT_TOKEN") >= 1,
            "grep should find edited base file"
        );
        // Grep: overflow content findable.
        assert!(
            grep_count(picker, "OVERFLOW_WIDGET_CONTENT") >= 1,
            "grep should find overflow file content"
        );
        // Grep: deleted file's content gone.
        assert_eq!(
            grep_count(picker, "SEED_TOKEN_0010"),
            0,
            "grep should not find deleted file content"
        );

        // Fuzzy: overflow file findable by name.
        let fuzzy = fuzzy_search_paths(picker, "overflow_widget");
        assert!(
            fuzzy.iter().any(|p| p.contains("unique_overflow_widget")),
            "fuzzy should find overflow file by name, got {fuzzy:?}"
        );

        // Fuzzy: deleted file not in results.
        let fuzzy_del = fuzzy_search_paths(picker, "file_0010");
        assert!(
            !fuzzy_del.iter().any(|p| p.contains("file_0010")),
            "fuzzy should not find deleted file"
        );

        // Fuzzy: edited file still findable by name.
        let fuzzy_edit = fuzzy_search_paths(picker, "file_0005");
        assert!(
            fuzzy_edit.iter().any(|p| p.contains("file_0005")),
            "fuzzy should find edited file by name"
        );
    }

    stop_picker(&shared_picker);
}

// HELPERS  --------------------------

fn grep_opts() -> GrepSearchOptions {
    GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: 500,
        mode: GrepMode::PlainText,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
    }
}

fn grep_count(picker: &FilePicker, query: &str) -> usize {
    let parsed = parse_grep_query(query);
    picker.grep(&parsed, &grep_opts()).matches.len()
}

fn grep_without_overlay_count(picker: &FilePicker, query: &str) -> usize {
    let parsed = parse_grep_query(query);
    picker
        .grep_without_overlay(&parsed, &grep_opts())
        .matches
        .len()
}

/// Wait for scanning to finish (no bigram requirement).
/// Use after `trigger_rescan` which replaces sync_data but does not
/// rebuild the bigram index.
fn wait_for_scan(shared_picker: &SharedPicker) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let ready = shared_picker
            .read()
            .ok()
            .map(|guard| guard.as_ref().map_or(false, |p| !p.is_scan_active()))
            .unwrap_or(false);
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for scan to complete"
        );
    }
}

fn wait_for_bigram(shared_picker: &SharedPicker) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let ready = shared_picker
            .read()
            .ok()
            .map(|guard| {
                guard
                    .as_ref()
                    .map_or(false, |p| !p.is_scan_active() && p.bigram_index().is_some())
            })
            .unwrap_or(false);
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for bigram build"
        );
    }
}

fn stop_picker(shared_picker: &SharedPicker) {
    if let Ok(mut guard) = shared_picker.write() {
        if let Some(ref mut picker) = *guard {
            picker.stop_background_monitor();
        }
    }
}

fn git_run(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed: {}", args, e));
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_init_and_commit(dir: &Path) {
    git_run(dir, &["init"]);
    git_run(dir, &["add", "-A"]);
    git_run(dir, &["commit", "-m", "initial"]);
}

fn git_add_and_commit(dir: &Path, msg: &str) {
    git_run(dir, &["add", "-A"]);
    git_run(dir, &["commit", "-m", msg]);
}

fn make_picker(base: &Path) -> (SharedPicker, SharedFrecency) {
    let shared_picker = SharedPicker::default();
    let shared_frecency = SharedFrecency::default();

    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: base.to_string_lossy().to_string(),
            warmup_mmap_cache: true,
            mode: FFFMode::Neovim,
            watch: false, // we drive events manually
            ..Default::default()
        },
    )
    .expect("Failed to create FilePicker");

    (shared_picker, shared_frecency)
}

/// Generate a file with a unique token embedded.
fn write_file_with_token(dir: &Path, name: &str, token: &str) {
    let content = format!(
        "// auto-generated file: {name}\nconst TOKEN: &str = \"{token}\";\nfn main() {{}}\n"
    );
    fs::write(dir.join(name), content).unwrap();
}

/// Generate many files with predictable tokens.
fn seed_files(dir: &Path, count: usize) -> Vec<(String, String)> {
    let mut files = Vec::with_capacity(count);
    for i in 0..count {
        let name = format!("file_{:04}.rs", i);
        let token = format!("SEED_TOKEN_{:04}", i);
        write_file_with_token(dir, &name, &token);
        files.push((name, token));
    }
    files
}
