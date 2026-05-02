use ahash::AHashMap;
use smallvec::SmallVec;
use std::borrow::Cow;

/// SIMD chunk size in bytes (matches NEON/SSE2 register width).
/// This must stay in sync with neo_frizbee's internal chunk size.
pub(crate) const SIMD_CHUNK_BYTES: usize = 16;

/// 4 chunks = 64 bytes inline, covers ~85% of paths without heap fallback.
const INLINE_CHUNKS: usize = 4;

pub(crate) type ChunkIndices = SmallVec<[u32; INLINE_CHUNKS]>;

#[derive(Clone, Copy)]
pub struct ArenaPtr(pub(crate) *const u8);

// SAFETY: The arena is a read-only immutable part of file sync
unsafe impl Send for ArenaPtr {}
unsafe impl Sync for ArenaPtr {}

impl ArenaPtr {
    #[inline]
    pub fn new(ptr: *const u8) -> Self {
        Self(ptr)
    }

    #[inline]
    pub fn null() -> Self {
        Self(std::ptr::null())
    }

    #[inline]
    pub fn as_ptr(self) -> *const u8 {
        self.0
    }
}

impl std::fmt::Debug for ArenaPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "--arena-raw-pointer-0x({:?})", self.0)
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(crate) struct SimdChunk(pub(crate) [u8; SIMD_CHUNK_BYTES]);

impl Default for SimdChunk {
    #[inline]
    fn default() -> Self {
        Self([0u8; SIMD_CHUNK_BYTES])
    }
}

impl std::fmt::Debug for SimdChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Show the actual bytes, trimming trailing zeros for readability
        let end = self.0.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        write!(f, "SimdChunk({:?})", &self.0[..end])
    }
}

pub const PATH_BUF_SIZE: usize = 4096;

/// Indices into a shared `SimdChunk` arena representing a file path.
///
/// All read methods require an explicit `arena_base` pointer from the owning
/// `ChunkedPathStore`. The struct itself contains no raw pointers to the arena
#[derive(Clone)]
pub(crate) struct ChunkedString {
    indices: ChunkIndices,
    pub byte_len: u16,
    /// Byte offset where the filename begins. 0 for root-level files.
    pub filename_offset: u16,
}

impl ChunkedString {
    pub fn empty() -> Self {
        Self {
            indices: SmallVec::new(),
            byte_len: 0,
            filename_offset: 0,
        }
    }

    #[inline]
    pub fn new(indices: ChunkIndices, byte_len: u16, filename_offset: u16) -> Self {
        Self {
            indices,
            byte_len,
            filename_offset,
        }
    }

    #[cfg(test)]
    pub fn chunk_count(&self) -> usize {
        self.indices.len()
    }

    #[inline]
    pub fn resolve_ptrs<'a>(
        &self,
        arena: ArenaPtr,
        buf: &'a mut [*const u8; 32],
    ) -> &'a [*const u8] {
        let count = self.indices.len();
        let base = arena.as_ptr();
        for (i, &idx) in self.indices.iter().enumerate() {
            buf[i] = unsafe { base.add(idx as usize * SIMD_CHUNK_BYTES) };
        }
        &buf[..count]
    }

    #[inline]
    fn write_slice_to_vec(
        indices: &[u32],
        base: *const u8,
        offset_in_chunk: usize,
        len: usize,
        vec: &mut Vec<u8>,
    ) {
        let mut written = 0usize;
        for (i, &idx) in indices.iter().enumerate() {
            let src = unsafe { base.add(idx as usize * SIMD_CHUNK_BYTES) };
            let chunk_bytes = unsafe { core::slice::from_raw_parts(src, SIMD_CHUNK_BYTES) };
            let start = if i == 0 { offset_in_chunk } else { 0 };
            let end = SIMD_CHUNK_BYTES.min(start + (len - written));
            vec.extend_from_slice(&chunk_bytes[start..end]);
            written += end - start;
        }
    }

    /// Return the filename portion as a `Cow<str>`.
    ///
    /// When the filename starts at a chunk boundary and fits in one chunk we
    /// borrow directly from the arena (zero-copy). Otherwise we allocate.
    /// Filenames are almost always <=16 bytes so the fast path dominates.
    #[inline]
    pub fn filename_cow<'a>(&self, arena: ArenaPtr) -> Cow<'a, str> {
        let fname_offset = self.filename_offset as usize;
        let fname_len = self.byte_len as usize - fname_offset;
        if fname_len == 0 {
            return Cow::Borrowed("");
        }

        let base = arena.as_ptr();
        let start_chunk = fname_offset / SIMD_CHUNK_BYTES;
        let offset_in_chunk = fname_offset % SIMD_CHUNK_BYTES;

        if offset_in_chunk == 0 && fname_len <= SIMD_CHUNK_BYTES {
            let ptr = unsafe { base.add(self.indices[start_chunk] as usize * SIMD_CHUNK_BYTES) };
            let slice = unsafe { core::slice::from_raw_parts(ptr, fname_len) };
            return Cow::Borrowed(unsafe { core::str::from_utf8_unchecked(slice) });
        }

        let mut out = String::with_capacity(fname_len);
        let needed_chunks = chunks_needed(offset_in_chunk + fname_len);
        Self::write_slice_to_vec(
            &self.indices[start_chunk..start_chunk + needed_chunks],
            base,
            offset_in_chunk,
            fname_len,
            unsafe { out.as_mut_vec() },
        );
        Cow::Owned(out)
    }

    /// Truncates at `buf.len()` if exceeded -- use `[u8; PATH_BUF_SIZE]` to avoid.
    #[inline]
    pub fn read_to_buf<'a>(&self, arena: ArenaPtr, buf: &'a mut [u8]) -> &'a str {
        let total = (self.byte_len as usize).min(buf.len());
        let usable_chunks = total.div_ceil(SIMD_CHUNK_BYTES);
        let chunks_to_copy = usable_chunks.min(self.indices.len());
        let base = arena.as_ptr();
        for (i, &idx) in self.indices[..chunks_to_copy].iter().enumerate() {
            let src = unsafe { base.add(idx as usize * SIMD_CHUNK_BYTES) };
            let dst_offset = i * SIMD_CHUNK_BYTES;
            let take = SIMD_CHUNK_BYTES.min(total - dst_offset);
            unsafe {
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr().add(dst_offset), take);
            }
        }

        unsafe { core::str::from_utf8_unchecked(&buf[..total]) }
    }

    #[inline]
    pub fn write_dir_to(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();

        let dir_len = self.filename_offset as usize;
        out.reserve(dir_len);
        let dir_chunks = chunks_needed(dir_len).min(self.indices.len());
        let base = arena.as_ptr();
        let vec = unsafe { out.as_mut_vec() };
        for (i, &idx) in self.indices[..dir_chunks].iter().enumerate() {
            let src = unsafe { base.add(idx as usize * SIMD_CHUNK_BYTES) };
            let take = SIMD_CHUNK_BYTES.min(dir_len - i * SIMD_CHUNK_BYTES);
            vec.extend_from_slice(unsafe { core::slice::from_raw_parts(src, take) });
        }
    }

    #[inline]
    pub fn write_filename_to(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();

        let fname_offset = self.filename_offset as usize;
        let fname_len = self.byte_len as usize - fname_offset;
        out.reserve(fname_len);
        let start_chunk = fname_offset / SIMD_CHUNK_BYTES;
        let offset_in_chunk = fname_offset % SIMD_CHUNK_BYTES;
        let needed_chunks = chunks_needed(offset_in_chunk + fname_len);
        Self::write_slice_to_vec(
            &self.indices[start_chunk..start_chunk + needed_chunks],
            arena.as_ptr(),
            offset_in_chunk,
            fname_len,
            unsafe { out.as_mut_vec() },
        );
    }

    #[inline]
    pub fn write_to_string(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();

        let total = self.byte_len as usize;
        if total == 0 {
            return;
        }
        out.reserve(total);
        let base = arena.as_ptr();
        let vec = unsafe { out.as_mut_vec() };
        for (i, &idx) in self.indices.iter().enumerate() {
            let src = unsafe { base.add(idx as usize * SIMD_CHUNK_BYTES) };
            let take = SIMD_CHUNK_BYTES.min(total - i * SIMD_CHUNK_BYTES);
            vec.extend_from_slice(unsafe { core::slice::from_raw_parts(src, take) });
        }
    }
}

impl std::fmt::Debug for ChunkedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkedString")
            .field("indices", &self.indices.as_slice())
            .field("chunks", &self.indices.len())
            .field("byte_len", &self.byte_len)
            .field("filename_offset", &self.filename_offset)
            .finish()
    }
}

#[inline]
const fn chunks_needed(byte_len: usize) -> usize {
    if byte_len == 0 {
        0
    } else {
        byte_len.div_ceil(SIMD_CHUNK_BYTES)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ChunkedPathStore {
    arena: Vec<SimdChunk>,
}

// SAFETY: arena is immutable after construction. Pointers derived from it are
// only read during scoring (no mutation, no reallocation).
unsafe impl Send for ChunkedPathStore {}
unsafe impl Sync for ChunkedPathStore {}

impl ChunkedPathStore {
    pub fn heap_bytes(&self) -> usize {
        self.arena.len() * SIMD_CHUNK_BYTES
    }

    #[cfg(test)]
    fn unique_chunks(&self) -> usize {
        self.arena.len()
    }

    #[inline]
    pub fn as_arena_ptr(&self) -> ArenaPtr {
        ArenaPtr::new(self.arena.as_ptr() as *const u8)
    }
}

/// At runtime the builder should be split out from the store after `finish()`.
#[derive(Clone, Debug)]
pub(crate) struct ChunkedPathStoreBuilder {
    arena: Vec<SimdChunk>,
    chunk_dedup: AHashMap<[u8; SIMD_CHUNK_BYTES], u32>,
}

impl ChunkedPathStoreBuilder {
    pub fn new(estimated_files: usize) -> Self {
        let est_chunks = estimated_files * 3;
        Self {
            arena: Vec::with_capacity(est_chunks / 2),
            chunk_dedup: AHashMap::with_capacity(est_chunks / 2),
        }
    }

    pub fn finish(self) -> ChunkedPathStore {
        ChunkedPathStore { arena: self.arena }
    }

    pub fn as_arena_ptr(&self) -> ArenaPtr {
        ArenaPtr::new(self.arena.as_ptr() as *const u8)
    }

    /// Like [`add_file_immediate`] but for directory paths where the entire
    /// string is the "directory" portion (filename_offset == byte_len).
    pub fn add_dir_immediate(&mut self, dir_rel_path: &str) -> ChunkedString {
        self.add_file_immediate(dir_rel_path, dir_rel_path.len() as u16)
    }

    pub fn add_file_immediate(&mut self, rel_path: &str, filename_offset: u16) -> ChunkedString {
        let path_bytes = rel_path.as_bytes();
        let byte_len = rel_path.len();
        let mut indices = ChunkIndices::with_capacity(chunks_needed(byte_len));

        for chunk in path_bytes.chunks(SIMD_CHUNK_BYTES) {
            let mut chunk_bytes = [0u8; SIMD_CHUNK_BYTES];
            chunk_bytes[..chunk.len()].copy_from_slice(chunk);

            let arena_idx = match self.chunk_dedup.get(&chunk_bytes) {
                Some(&idx) => idx,
                None => {
                    let idx = self.arena.len() as u32;
                    self.arena.push(SimdChunk(chunk_bytes));
                    self.chunk_dedup.insert(chunk_bytes, idx);
                    idx
                }
            };

            indices.push(arena_idx);
        }

        ChunkedString::new(indices, byte_len as u16, filename_offset)
    }
}

#[cfg(test)]
pub(crate) fn build_chunked_path_store_from_strings(
    rel_paths: &[String],
    files: &[crate::types::FileItem],
) -> (ChunkedPathStore, Vec<ChunkedString>) {
    assert_eq!(rel_paths.len(), files.len());
    let mut builder = ChunkedPathStoreBuilder::new(rel_paths.len());
    let strings: Vec<ChunkedString> = rel_paths
        .iter()
        .zip(files.iter())
        .map(|(rel_path, file)| builder.add_file_immediate(rel_path, file.path.filename_offset))
        .collect();
    (builder.finish(), strings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file_item(path: &str) -> crate::types::FileItem {
        let filename_start = path
            .rfind(std::path::is_separator)
            .map(|i| i + 1)
            .unwrap_or(0) as u16;
        crate::types::FileItem::new_raw(filename_start, 0, 0, None, false)
    }

    fn build_test_store(
        paths: &[&str],
    ) -> (
        ChunkedPathStore,
        Vec<ChunkedString>,
        Vec<crate::types::FileItem>,
    ) {
        let mut files: Vec<crate::types::FileItem> =
            paths.iter().map(|p| make_file_item(p)).collect();
        let path_strings: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let (store, strings) = build_chunked_path_store_from_strings(&path_strings, &files);
        for (i, file) in files.iter_mut().enumerate() {
            file.set_path(strings[i].clone());
        }
        (store, strings, files)
    }

    #[test]
    fn test_chunked_store_empty() {
        let (store, strings, _files) = build_test_store(&[]);
        assert_eq!(strings.len(), 0);
        assert_eq!(store.unique_chunks(), 0);
    }

    #[test]
    fn test_chunked_store_basic() {
        let (store, strings, _files) =
            build_test_store(&["src/lib.rs", "src/main.rs", "Cargo.toml"]);
        let arena = store.as_arena_ptr();

        assert_eq!(strings.len(), 3);
        assert!(store.unique_chunks() >= 2);

        let mut buf = [0u8; 512];
        assert_eq!(
            strings[0].read_to_buf(arena, &mut buf).len(),
            "src/lib.rs".len()
        );
        assert_eq!(
            strings[2].read_to_buf(arena, &mut buf).len(),
            "Cargo.toml".len()
        );
    }

    #[test]
    fn test_chunked_string_full_path() {
        let (store, strings, _files) = build_test_store(&["src/components/Button.tsx"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        let mut buf = [0u8; 512];
        assert_eq!(cs.read_to_buf(arena, &mut buf), "src/components/Button.tsx");
        assert_eq!(cs.byte_len, 25);
        assert_eq!(cs.filename_offset, 15);
    }

    #[test]
    fn test_chunked_string_dir_and_filename() {
        let (store, strings, _files) = build_test_store(&["src/components/Button.tsx"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        let mut s = String::new();
        cs.write_dir_to(arena, &mut s);
        assert_eq!(s, "src/components/");
        cs.write_filename_to(arena, &mut s);
        assert_eq!(s, "Button.tsx");
    }

    #[test]
    fn test_chunked_string_root_file() {
        let (store, strings, _files) = build_test_store(&["Cargo.toml"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        let mut s = String::new();
        cs.write_dir_to(arena, &mut s);
        assert_eq!(s, "");
        cs.write_filename_to(arena, &mut s);
        assert_eq!(s, "Cargo.toml");
        let mut buf = [0u8; 512];
        assert_eq!(cs.read_to_buf(arena, &mut buf), "Cargo.toml");
    }

    #[test]
    fn test_chunked_string_resolve_ptrs() {
        let (store, strings, _files) = build_test_store(&["src/components/Button.tsx"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        let mut ptrs = [std::ptr::null::<u8>(); 32];
        let resolved = cs.resolve_ptrs(arena, &mut ptrs);
        assert_eq!(resolved.len(), 2); // 25 bytes = 2 chunks

        // Verify we can read back the bytes
        let mut reconstructed = Vec::new();
        for (i, &ptr) in resolved.iter().enumerate() {
            let chunk = unsafe { std::slice::from_raw_parts(ptr, SIMD_CHUNK_BYTES) };
            let start = i * SIMD_CHUNK_BYTES;
            let take = SIMD_CHUNK_BYTES.min(25 - start);
            reconstructed.extend_from_slice(&chunk[..take]);
        }
        assert_eq!(
            std::str::from_utf8(&reconstructed).unwrap(),
            "src/components/Button.tsx"
        );
    }

    #[test]
    fn test_filename_cow_mid_chunk() {
        let (store, strings, _files) = build_test_store(&["src/components/Button.tsx"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        assert_eq!(cs.filename_offset, 15);
        assert_eq!(cs.byte_len, 25);

        let fname = cs.filename_cow(arena);
        assert_eq!(&*fname, "Button.tsx");
    }

    #[test]
    fn test_filename_cow_chunk_aligned() {
        let path = "0123456789abcdef/file.txt";
        let (store, strings, _files) = build_test_store(&[path]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        assert_eq!(cs.filename_offset, 17);
        let fname = cs.filename_cow(arena);
        assert_eq!(&*fname, "file.txt");
    }

    #[test]
    fn test_filename_cow_root_file() {
        let (store, strings, _files) = build_test_store(&["Cargo.toml"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        assert_eq!(cs.filename_offset, 0);
        let fname = cs.filename_cow(arena);
        assert_eq!(&*fname, "Cargo.toml");
    }

    #[test]
    fn test_chunked_string_long_path() {
        let path = "very/deeply/nested/directory/structure/with/many/levels/file.txt";
        let (store, strings, _files) = build_test_store(&[path]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];

        let mut buf = [0u8; 512];
        assert_eq!(cs.read_to_buf(arena, &mut buf), path);
        assert!(
            cs.chunk_count() <= 6,
            "should fit inline in ChunkIndices (INLINE_CHUNKS={})",
            INLINE_CHUNKS
        );
    }

    #[test]
    fn test_chunked_string_clone() {
        let (store, strings, _files) = build_test_store(&["src/main.rs"]);
        let arena = store.as_arena_ptr();
        let cs = &strings[0];
        let cs2 = cs.clone();

        let mut buf1 = [0u8; 512];
        let mut buf2 = [0u8; 512];
        assert_eq!(
            cs.read_to_buf(arena, &mut buf1),
            cs2.read_to_buf(arena, &mut buf2)
        );
    }

    #[test]
    fn test_chunked_string_full_path_roundtrip() {
        let paths = [
            "src/components/Button.tsx",
            "src/components/ui/DatePicker.tsx",
            "very/deeply/nested/directory/structure/file.txt",
            "Cargo.toml",
            "a.rs",
        ];
        let (store, strings, _files) = build_test_store(&paths);
        let arena = store.as_arena_ptr();

        for (i, expected) in paths.iter().enumerate() {
            let mut buf = [0u8; 512];
            let got = strings[i].read_to_buf(arena, &mut buf);
            assert_eq!(got, *expected, "full path roundtrip failed for file {i}");

            let mut ds = String::new();
            let mut fs = String::new();
            strings[i].write_dir_to(arena, &mut ds);
            strings[i].write_filename_to(arena, &mut fs);
            assert_eq!(
                format!("{ds}{fs}"),
                *expected,
                "dir+fname mismatch for file {i}"
            );
        }
    }
}
