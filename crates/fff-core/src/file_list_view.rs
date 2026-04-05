//! Zero-copy file list backed by a memory-mapped buffer.
//!
//! [`FileRecord`] is a `repr(C)` fixed-size record describing one file.
//! [`FileListView`] holds an array of records plus a string table, both
//! borrowed from an mmap. Provides indexed access to file metadata
//! without constructing owned [`FileItem`](crate::types::FileItem)s.

use crate::types::FileItem;
use std::path::{Path, PathBuf};

/// Fixed-size, `repr(C)` file metadata record for mmap-friendly storage.
///
/// Fields are ordered to avoid padding on both 32-bit and 64-bit platforms.
/// The `name_len` high bit stores the `is_binary` flag (max component length
/// is 255 on most filesystems, so 15 bits is sufficient).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FileRecord {
    /// Byte offset of `relative_path` in the string table.
    pub path_offset: u32,
    /// Length of `relative_path` in bytes.
    pub path_len: u16,
    /// Length of `file_name` (the last path component). High bit = is_binary.
    pub name_len_and_flags: u16,
    /// File size in bytes.
    pub size: u64,
    /// Last modification time (seconds since UNIX epoch).
    pub modified: u64,
}

const BINARY_FLAG: u16 = 0x8000;

impl FileRecord {
    /// Create a new record.
    pub fn new(
        path_offset: u32,
        path_len: u16,
        name_len: u16,
        is_binary: bool,
        size: u64,
        modified: u64,
    ) -> Self {
        let mut flags = name_len;
        if is_binary {
            flags |= BINARY_FLAG;
        }
        Self {
            path_offset,
            path_len,
            name_len_and_flags: flags,
            size,
            modified,
        }
    }

    /// Length of the file name component.
    #[inline]
    pub fn name_len(&self) -> u16 {
        self.name_len_and_flags & !BINARY_FLAG
    }

    /// Whether the file was detected as binary.
    #[inline]
    pub fn is_binary(&self) -> bool {
        self.name_len_and_flags & BINARY_FLAG != 0
    }

    /// Size of this struct in bytes (for serialization stride).
    pub const SIZE: usize = std::mem::size_of::<Self>();
}

// Verify repr(C) layout is what we expect.
const _: () = assert!(FileRecord::SIZE == 24);

/// Read-only view over a flat file list stored as [`FileRecord`]s + a string table.
///
/// Both the records and strings are borrowed — typically from an mmap.
/// This avoids all heap allocation during load. Call [`to_file_items`]
/// when you need owned [`FileItem`]s for the search pipeline.
pub struct FileListView<'a> {
    records: &'a [FileRecord],
    strings: &'a [u8],
}

impl<'a> FileListView<'a> {
    /// Construct a view from raw record and string table slices.
    ///
    /// # Safety
    /// The caller must ensure `records` was produced from a `repr(C)`
    /// `FileRecord` array and `strings` contains valid UTF-8 at all
    /// offsets referenced by the records.
    pub unsafe fn new(records: &'a [FileRecord], strings: &'a [u8]) -> Self {
        Self { records, strings }
    }

    /// Number of files in this view.
    #[inline]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the view is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Get the record at index `i`.
    #[inline]
    pub fn record(&self, i: usize) -> &FileRecord {
        &self.records[i]
    }

    /// Get the relative path for file `i` as a `&str`.
    #[inline]
    pub fn relative_path(&self, i: usize) -> &'a str {
        let r = &self.records[i];
        let start = r.path_offset as usize;
        let end = start + r.path_len as usize;
        // SAFETY: caller guarantees valid UTF-8.
        unsafe { std::str::from_utf8_unchecked(&self.strings[start..end]) }
    }

    /// Get the file name for file `i` (last component of relative path).
    #[inline]
    pub fn file_name(&self, i: usize) -> &'a str {
        let r = &self.records[i];
        let name_len = r.name_len() as usize;
        let path_start = r.path_offset as usize;
        let path_end = path_start + r.path_len as usize;
        // SAFETY: name is a suffix of relative_path, both valid UTF-8.
        unsafe { std::str::from_utf8_unchecked(&self.strings[path_end - name_len..path_end]) }
    }

    /// Convert to owned `FileItem`s for the search pipeline.
    /// Allocates strings and PathBufs from a sequential scan of the mmap'd data.
    pub fn to_file_items(&self, base_path: &Path) -> Vec<FileItem> {
        let base_bytes = base_path.as_os_str().as_encoded_bytes();
        let mut items = Vec::with_capacity(self.records.len());

        for i in 0..self.records.len() {
            let r = &self.records[i];
            let path_bytes = &self.strings
                [r.path_offset as usize..(r.path_offset as usize + r.path_len as usize)];
            let name_len = r.name_len() as usize;

            let relative_path = unsafe { String::from_utf8_unchecked(path_bytes.to_vec()) };
            let file_name = if name_len > 0 && name_len <= path_bytes.len() {
                unsafe {
                    String::from_utf8_unchecked(path_bytes[path_bytes.len() - name_len..].to_vec())
                }
            } else {
                relative_path.clone()
            };

            let mut full = Vec::with_capacity(base_bytes.len() + 1 + path_bytes.len());
            full.extend_from_slice(base_bytes);
            full.push(b'/');
            full.extend_from_slice(path_bytes);
            let full_path =
                PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(full) });

            items.push(FileItem::new_raw(
                full_path,
                relative_path,
                file_name,
                r.size,
                r.modified,
                None,
                r.is_binary(),
            ));
        }

        items
    }
}

/// Build a `FileRecord` array and string table from a slice of `FileItem`s.
///
/// Returns `(records, string_table)` suitable for writing to disk or
/// constructing a `FileListView`.
pub fn build_file_records(files: &[FileItem]) -> (Vec<FileRecord>, Vec<u8>) {
    let string_table_size: usize = files.iter().map(|f| f.relative_path.len()).sum();
    let mut records = Vec::with_capacity(files.len());
    let mut strings = Vec::with_capacity(string_table_size);

    for file in files {
        let path_offset = strings.len() as u32;
        let path_len = file.relative_path.len() as u16;
        let name_len = file.file_name.len() as u16;
        strings.extend_from_slice(file.relative_path.as_bytes());

        records.push(FileRecord::new(
            path_offset,
            path_len,
            name_len,
            file.is_binary,
            file.size,
            file.modified,
        ));
    }

    (records, strings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_record_size() {
        assert_eq!(FileRecord::SIZE, 24);
    }

    #[test]
    fn test_file_record_flags() {
        let r = FileRecord::new(0, 10, 5, false, 100, 200);
        assert_eq!(r.name_len(), 5);
        assert!(!r.is_binary());

        let r = FileRecord::new(0, 10, 5, true, 100, 200);
        assert_eq!(r.name_len(), 5);
        assert!(r.is_binary());
    }

    #[test]
    fn test_round_trip() {
        let files = vec![
            FileItem::new_raw(
                PathBuf::from("/base/src/main.rs"),
                "src/main.rs".to_string(),
                "main.rs".to_string(),
                1024,
                1000000,
                None,
                false,
            ),
            FileItem::new_raw(
                PathBuf::from("/base/tests/test.rs"),
                "tests/test.rs".to_string(),
                "test.rs".to_string(),
                512,
                2000000,
                None,
                false,
            ),
            FileItem::new_raw(
                PathBuf::from("/base/image.png"),
                "image.png".to_string(),
                "image.png".to_string(),
                8192,
                3000000,
                None,
                true,
            ),
        ];

        let (records, strings) = build_file_records(&files);
        let view = unsafe { FileListView::new(&records, &strings) };

        assert_eq!(view.len(), 3);

        assert_eq!(view.relative_path(0), "src/main.rs");
        assert_eq!(view.file_name(0), "main.rs");
        assert_eq!(view.record(0).size, 1024);
        assert_eq!(view.record(0).modified, 1000000);
        assert!(!view.record(0).is_binary());

        assert_eq!(view.relative_path(1), "tests/test.rs");
        assert_eq!(view.file_name(1), "test.rs");

        assert_eq!(view.relative_path(2), "image.png");
        assert!(view.record(2).is_binary());

        // Test to_file_items round-trip
        let items = view.to_file_items(Path::new("/base"));
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].relative_path, "src/main.rs");
        assert_eq!(items[0].file_name, "main.rs");
        assert_eq!(items[0].size, 1024);
        assert!(items[0].path.ends_with("src/main.rs"));
        assert!(!items[0].is_binary);
        assert!(items[2].is_binary);
    }
}
