/// A Vec wrapper that prevents silent reallocation.
///
/// The inner Vec is not exposed — all mutations go through checked
/// methods. `push()` returns `false` when capacity is exhausted,
/// making it structurally impossible to reallocate without an
/// explicit new construction.
#[derive(Clone)]
pub(crate) struct StableVec<T>(Vec<T>);

impl<T: std::fmt::Debug> std::fmt::Debug for StableVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("StableVec").field(&self.0.len()).finish()
    }
}

impl<T> StableVec<T> {
    pub fn from_vec_with_reserve(mut vec: Vec<T>, extra: usize) -> Self {
        vec.reserve(extra);
        Self(vec)
    }

    /// Push an item. Returns `false` if capacity is exhausted — the
    /// item is dropped and the caller should trigger a full rescan.
    /// In debug builds also panics to catch the logic error early.
    #[inline]
    pub fn push(&mut self, item: T) -> bool {
        if self.0.len() >= self.0.capacity() {
            debug_assert!(
                false,
                "StableVec: push would reallocate ({} at capacity {})",
                self.0.len(),
                self.0.capacity(),
            );
            tracing::error!(
                len = self.0.len(),
                capacity = self.0.capacity(),
                "StableVec: capacity exhausted — dropping item to prevent reallocation"
            );
            return false;
        }
        self.0.push(item);
        true
    }

    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        self.0.remove(index)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.0.as_ptr()
    }

    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.0.get_mut(index)
    }

    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.0.last()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.0.iter_mut()
    }

    #[inline]
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        self.0.retain(f);
    }
}

impl<T> std::ops::Deref for StableVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> std::ops::DerefMut for StableVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}
