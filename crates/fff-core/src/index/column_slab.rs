use std::ops::Deref;

/// Page-aligned `u64` buffer for bigram columns. On Unix it is a private
/// anonymous mmap so the builder slab bypasses the allocator and the unused
/// tail can be handed back with `munmap` after in-place compaction.
pub struct ColumnSlab {
    ptr: *mut u64,
    len: usize,
    #[cfg(unix)]
    mapped_bytes: usize,
    #[cfg(not(unix))]
    vec: Vec<u64>,
}

unsafe impl Send for ColumnSlab {}
unsafe impl Sync for ColumnSlab {}

impl ColumnSlab {
    pub fn zeroed(len: usize) -> Self {
        #[cfg(unix)]
        {
            let mapped_bytes = (len * 8).next_multiple_of(page_size()).max(page_size());
            // SAFETY: anonymous private mapping; the kernel hands back zero pages.
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    mapped_bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert!(
                ptr != libc::MAP_FAILED,
                "mmap of {mapped_bytes} bytes failed"
            );
            Self {
                ptr: ptr as *mut u64,
                len,
                mapped_bytes,
            }
        }
        #[cfg(not(unix))]
        {
            Self::from_vec(vec![0u64; len])
        }
    }

    pub fn from_vec(vec: Vec<u64>) -> Self {
        #[cfg(unix)]
        {
            let mut slab = Self::zeroed(vec.len());
            slab.as_mut_slice().copy_from_slice(&vec);
            slab
        }
        #[cfg(not(unix))]
        {
            let mut vec = vec;
            Self {
                ptr: vec.as_mut_ptr(),
                len: vec.len(),
                vec,
            }
        }
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u64 {
        self.ptr
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u64] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    /// Shrink to the first `len` words, returning the trailing pages to the OS.
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        self.len = len;
        #[cfg(unix)]
        {
            let keep = (len * 8).next_multiple_of(page_size()).max(page_size());
            if keep < self.mapped_bytes {
                // SAFETY: unmapping a page-aligned tail of our own mapping.
                unsafe {
                    libc::munmap(
                        (self.ptr as *mut u8).add(keep).cast(),
                        self.mapped_bytes - keep,
                    );
                }
                self.mapped_bytes = keep;
            }
        }
        #[cfg(not(unix))]
        {
            self.vec.truncate(len);
            self.vec.shrink_to_fit();
            self.ptr = self.vec.as_mut_ptr();
        }
    }
}

impl Deref for ColumnSlab {
    type Target = [u64];

    #[inline]
    fn deref(&self) -> &[u64] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

#[cfg(unix)]
impl Drop for ColumnSlab {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr.cast(), self.mapped_bytes);
        }
    }
}

impl std::fmt::Debug for ColumnSlab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnSlab")
            .field("words", &self.len)
            .finish()
    }
}

#[cfg(unix)]
fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeroed_truncate_and_roundtrip() {
        let mut slab = ColumnSlab::zeroed(10_000);
        assert!(slab.iter().all(|&w| w == 0));
        slab.as_mut_slice()[9_999] = 7;
        slab.as_mut_slice()[3] = 5;
        slab.truncate(4);
        assert_eq!(&slab[..], &[0, 0, 0, 5]);
        slab.truncate(100);
        assert_eq!(slab.len(), 4);

        let v = ColumnSlab::from_vec(vec![1, 2, 3]);
        assert_eq!(&v[..], &[1, 2, 3]);
        let empty = ColumnSlab::zeroed(0);
        assert!(empty.is_empty());
    }
}
