//! Regression test for https://github.com/dmtrKovalenko/fff/issues/799
//!
//! Unix filenames are arbitrary bytes. The zlob walker used to pair a raw-byte
//! basename offset with a lossily decoded path, so every invalid byte (3-byte
//! U+FFFD) shifted the real offset and the dir-table sweep sliced inside a
//! replacement char, panicking on the Rayon scan thread and aborting.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use fff_search::file_picker::FilePicker;
use fff_search::{FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser};

/// Byte-exact corpus from the reporter's tarball (fff-invalid-utf8-repro.tar.gz,
/// sha256 fc0011038d27bbd63a3ca67a929d4f342b4895f922062fe86bd958712c67de08):
/// four nested dirs plus one empty file, names carrying invalid UTF-8,
/// control bytes and truncated multibyte sequences.
const REPRO_COMPONENTS: &[(&str, bool)] = &[
    // (hex-encoded name, is_dir)
    ("64617461322e6361622e657874726163746564", true),
    (
        "2366711cf02a03386b398b0b95824b120cd90135e777c6356ab77ead67ed0be428e3c5a7c49084cd62468cef4f780b46a97ff5e2be8afefd0962a9cfae50f861a2f637fcfe62f44c32aa96392ce1873f392ce18785905893a6c8f7b9b80b39f9cacc576a22f42758c9bd1e3071bbf041ee629cf0bbd09771b2",
        true,
    ),
    (
        "33516e5b0214f6bdf7056a22dd6886262defcb33811eb9d4bf84b321422cd4b2beb87e5db8c4aedf7fae5e45a5c5fae7e9ba39b1547f8cc3778738f9a94391b3875f20c4cb0a1b4b163938f39558e66ab0e81d0bcacd4139fd62cbf3bcfd9142a29ab511ad9c7766390abad8143428261982ba754133515066a9edf0d34c13e33e35858f65eee4adb6144fdb11546fc442574121adb24191fb95ae6ee3c40a9d787cde8cf1941c8ca7e4f903f2a5b5eaee4b5fe5b18ad62b3a48dc67431d94e0d6503b8e7bf48cd7c3be9c8569598e6f23db3222",
        true,
    ),
    (
        "6248697c3afe892703961a9d668d9ac88f557d357768c5e665ac9abc505920a44c06a56fff",
        true,
    ),
    (
        "07fa04794121fad9f17a52a373a636bec7d08d0fa68e1e7a61ce8751bbf68d06ab74fe07206dde4cadb44730f93b49f2987148fe1f28093980f5df7fcbac6b46d9837e31e97e1bae9f581a665209e43f783f06a65666d1853a9f36dea3b56aa3dcfdf7c08ccdbd7423b460de92c7157f506c1f6057fab76abb2bdae6056ccadc49dac6f5ee1e8af2c499f60dc350e3df6a5b9f38135b5dbd60c182e099af1d1f6d3c9ba26dda282e77ec59bde5afcf3e1204f7f8fd8d67bb6bdb37b65cee783f888f8454cf5d967b239ee8cd956dc3e616",
        false,
    ),
];

/// Hand-made corpus covering the shapes the tarball doesn't: invalid bytes in
/// the leading component, an invalid byte right before the separator, a name
/// that is invalid only in its basename, and a valid-UTF-8 multibyte dir whose
/// offset must stay untouched. Entries are '/'-joined hex components.
const EXTRA_CASES: &[(&str, bool)] = &[
    // "\xff\xffx/file.txt" — the minimal case from the triage report
    ("ffff78/66696c652e747874", false),
    // "a\xffb\xff/plain.md" — invalid byte immediately before the separator
    ("61ff62ff/706c61696e2e6d64", false),
    // valid dir, invalid basename: "ok/\xc3(bad"
    ("6f6b/c328626164", false),
    // "каталог/main.rs" — valid multibyte, offset must equal the raw one
    ("d0bad0b0d182d0b0d0bbd0bed0b3/6d61696e2e7273", false),
];

#[test]
#[cfg(unix)]
fn scans_invalid_utf8_corpus_without_panicking() {
    let tmp = TempDir::new().unwrap();
    let Some(expected) = build_chain(tmp.path(), REPRO_COMPONENTS) else {
        eprintln!("filesystem rejects invalid UTF-8 names; skipping");
        return;
    };

    // Pre-fix this aborts the process from the background scan thread.
    let picker = scan(tmp.path());
    let indexed = all_paths(&picker);

    assert!(
        indexed.contains(&expected),
        "the reporter's file must be indexed under its lossy path"
    );
    assert_offsets_are_char_boundaries(&picker);
    assert_dirs_indexed(&picker, &expected);
}

#[test]
#[cfg(unix)]
fn scans_mixed_encoding_corpus_without_panicking() {
    let tmp = TempDir::new().unwrap();
    let Some(()) = build_paths(tmp.path(), EXTRA_CASES) else {
        eprintln!("filesystem rejects invalid UTF-8 names; skipping");
        return;
    };

    let picker = scan(tmp.path());
    let indexed = all_paths(&picker);

    assert!(
        indexed
            .iter()
            .any(|p| p.ends_with("\u{FFFD}\u{FFFD}x/file.txt")),
        "expected the \\xff\\xffx/file.txt case, got {indexed:?}"
    );
    // A valid-UTF-8 multibyte dir must keep the raw offset verbatim.
    let cyrillic = indexed
        .iter()
        .find(|p| p.contains("каталог"))
        .expect("cyrillic path must be indexed");
    assert!(cyrillic.ends_with("каталог/main.rs"), "got {cyrillic:?}");
    assert_offsets_are_char_boundaries(&picker);
}

/// Searching by a plain ASCII component of a path whose other components are
/// invalid UTF-8 must still work end to end.
#[test]
#[cfg(unix)]
fn searches_across_invalid_utf8_parents() {
    let tmp = TempDir::new().unwrap();
    if build_chain(tmp.path(), REPRO_COMPONENTS).is_none() {
        eprintln!("filesystem rejects invalid UTF-8 names; skipping");
        return;
    }

    let picker = scan(tmp.path());
    let parser = QueryParser::default();
    let parsed = parser.parse("data2.cab.extracted");
    let result = picker.fuzzy_search(
        &parsed,
        None,
        FuzzySearchOptions {
            max_threads: 1,
            pagination: PaginationArgs {
                offset: 0,
                limit: 50,
            },
            ..Default::default()
        },
    );

    assert_eq!(result.items.len(), 1, "the single corpus file must match");
    assert!(
        result.items[0]
            .relative_path(&picker)
            .starts_with("data2.cab.extracted/")
    );
}

fn scan(base: &Path) -> FilePicker {
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: base.to_string_lossy().into_owned(),
        enable_mmap_cache: false,
        watch: false,
        ..Default::default()
    })
    .expect("FilePicker::new");
    picker.collect_files().expect("collect_files");
    picker
}

fn all_paths(picker: &FilePicker) -> Vec<String> {
    picker
        .get_files()
        .iter()
        .map(|f| f.relative_path(picker))
        .collect()
}

/// The exact invariant #799 violated: the stored basename offset must be a
/// char boundary of the decoded path and must point right past a separator.
fn assert_offsets_are_char_boundaries(picker: &FilePicker) {
    for file in picker.get_files().iter() {
        let path = file.relative_path(picker);
        let offset = file.filename_offset_in_relative_path();
        assert!(
            path.is_char_boundary(offset),
            "offset {offset} is not a char boundary of {path:?}"
        );
        assert!(offset <= path.len(), "offset {offset} past end of {path:?}");
        if offset > 0 {
            assert_eq!(
                &path[offset - 1..offset],
                "/",
                "offset must follow a separator in {path:?}"
            );
        }
        assert!(
            !path[offset..].contains('/'),
            "basename split wrong: {path:?}"
        );
    }
}

/// Every ancestor of the corpus file must land in the searchable dir table.
fn assert_dirs_indexed(picker: &FilePicker, file_path: &str) {
    let dirs = picker
        .get_dirs()
        .iter()
        .map(|d| d.relative_path(picker))
        .collect::<Vec<_>>();

    let mut prefix = String::new();
    let components: Vec<&str> = file_path.split('/').collect();
    for component in &components[..components.len() - 1] {
        prefix.push_str(component);
        prefix.push('/');
        assert!(
            dirs.iter()
                .any(|d| d.trim_end_matches('/') == prefix.trim_end_matches('/')),
            "missing dir {prefix:?} in {dirs:?}"
        );
    }
}

/// Materializes the tarball chain under `root`: each entry nests inside the
/// previous one. Returns the '/'-joined lossy path of the leaf file, or `None`
/// when the filesystem refuses invalid UTF-8 names (APFS returns EILSEQ, ZFS
/// with utf8only likewise) so the test skips instead of failing.
#[cfg(unix)]
fn build_chain(root: &Path, components: &[(&str, bool)]) -> Option<String> {
    let mut path = PathBuf::from(root);
    let mut decoded = String::new();

    for (hex, is_dir) in components {
        let bytes = unhex(hex);
        path.push(os_str(&bytes));
        if !decoded.is_empty() {
            decoded.push('/');
        }
        decoded.push_str(&String::from_utf8_lossy(&bytes));

        if !create(&path, *is_dir) {
            return None;
        }
    }

    Some(decoded)
}

/// Materializes independent '/'-joined hex paths under `root`.
#[cfg(unix)]
fn build_paths(root: &Path, specs: &[(&str, bool)]) -> Option<()> {
    for (hex, is_dir) in specs {
        let mut path = PathBuf::from(root);
        let components: Vec<Vec<u8>> = hex.split('/').map(unhex).collect();
        let (last, parents) = components.split_last().unwrap();

        for parent in parents {
            path.push(os_str(parent));
            if !create(&path, true) {
                return None;
            }
        }

        path.push(os_str(last));
        if !create(&path, *is_dir) {
            return None;
        }
    }
    Some(())
}

#[cfg(unix)]
fn os_str(bytes: &[u8]) -> &std::ffi::OsStr {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(bytes)
}

#[cfg(unix)]
fn create(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        std::fs::create_dir_all(path).is_ok()
    } else {
        std::fs::write(path, b"").is_ok()
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
