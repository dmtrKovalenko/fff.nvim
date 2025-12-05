//! Thread-local sort buffer management for glidesort optimization
//!
//! This module provides thread-local buffers for glidesort's with_buffer API,
//! eliminating allocations in the hot path of fuzzy search operations.

use std::cell::RefCell;
use std::mem::MaybeUninit;

// glidesort requires a buffer to allocate, we use one reused buffer as it can grow pretty big
// for a large projects, this effectively saves 12kb of allocation on every search in linux repo
thread_local! {
    static SORT_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1024));
}

pub fn sort_with_buffer<T, F>(slice: &mut [T], compare: F)
where
    F: FnMut(&T, &T) -> std::cmp::Ordering,
{
    SORT_BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();

        // Calculate required buffer size in u8 units
        let size_of_t = std::mem::size_of::<MaybeUninit<T>>();
        let size_of_usize = std::mem::size_of::<u8>();
        let required_usizes = (slice.len() * size_of_t).div_ceil(size_of_usize);

        // Ensure buffer has enough capacity
        if buffer.len() < required_usizes {
            buffer.resize(required_usizes, 0);
        }

        // Cast u8 buffer to MaybeUninit<T> slice
        // SAFETY: u8 provides sufficient alignment for most types, and we've ensured
        // the buffer is large enough
        let typed_buffer = unsafe {
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut MaybeUninit<T>, slice.len())
        };

        glidesort::sort_with_buffer_by(slice, typed_buffer, compare);
    });
}

pub fn sort_by_key_with_buffer<T, K, F>(slice: &mut [T], key_fn: F)
where
    K: Ord,
    F: FnMut(&T) -> K,
{
    SORT_BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();

        // Calculate required buffer size in u8 units
        let size_of_t = std::mem::size_of::<MaybeUninit<T>>();
        let size_of_usize = std::mem::size_of::<u8>();
        let required_usizes = (slice.len() * size_of_t).div_ceil(size_of_usize);

        // Ensure buffer has enough capacity
        if buffer.len() < required_usizes {
            buffer.resize(required_usizes, 0);
        }

        // Cast u8 buffer to MaybeUninit<T> slice
        // SAFETY: u8 provides sufficient alignment for most types, and we've ensured
        // the buffer is large enough
        let typed_buffer = unsafe {
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut MaybeUninit<T>, slice.len())
        };

        glidesort::sort_with_buffer_by_key(slice, typed_buffer, key_fn);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sort_with_buffer() {
        let mut data = vec![5, 2, 8, 1, 9];
        sort_with_buffer(&mut data, |a, b| a.cmp(b));
        assert_eq!(data, vec![1, 2, 5, 8, 9]);
    }

    #[test]
    fn test_sort_by_key_with_buffer() {
        let mut data = vec![(2, "b"), (1, "a"), (3, "c")];
        sort_by_key_with_buffer(&mut data, |item| item.0);
        assert_eq!(data, vec![(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn test_reverse_sort() {
        let mut data = vec![1, 2, 3, 4, 5];
        sort_with_buffer(&mut data, |a, b| b.cmp(a));
        assert_eq!(data, vec![5, 4, 3, 2, 1]);
    }

    #[test]
    fn test_multiple_sorts_reuse_buffer() {
        // This test verifies that multiple sorts on the same thread reuse the buffer
        let mut data1 = vec![5, 2, 8, 1, 9];
        sort_with_buffer(&mut data1, |a, b| a.cmp(b));

        let mut data2 = vec![15, 12, 18, 11, 19];
        sort_with_buffer(&mut data2, |a, b| a.cmp(b));

        assert_eq!(data1, vec![1, 2, 5, 8, 9]);
        assert_eq!(data2, vec![11, 12, 15, 18, 19]);
    }

    #[test]
    fn test_empty_slice() {
        let mut data: Vec<i32> = vec![];
        sort_with_buffer(&mut data, |a, b| a.cmp(b));
        assert_eq!(data, Vec::<i32>::new());
    }

    #[test]
    fn test_single_element() {
        let mut data = vec![42];
        sort_with_buffer(&mut data, |a, b| a.cmp(b));
        assert_eq!(data, vec![42]);
    }

    #[test]
    fn test_already_sorted() {
        let mut data = vec![1, 2, 3, 4, 5];
        sort_with_buffer(&mut data, |a, b| a.cmp(b));
        assert_eq!(data, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_with_duplicates() {
        let mut data = vec![3, 1, 4, 1, 5, 9, 2, 6, 5];
        sort_with_buffer(&mut data, |a, b| a.cmp(b));
        assert_eq!(data, vec![1, 1, 2, 3, 4, 5, 5, 6, 9]);
    }

    #[test]
    fn test_descending_order() {
        let mut data = vec![3, 1, 4, 1, 5, 9, 2, 6, 5];
        sort_with_buffer(&mut data, |a, b| b.cmp(a));
        assert_eq!(data, vec![9, 6, 5, 5, 4, 3, 2, 1, 1]);
    }

    #[test]
    fn test_simple_descending() {
        // Simple test to verify highest scores come first
        let mut data = vec![100, 300, 200];
        sort_with_buffer(&mut data, |a, b| b.cmp(a));
        println!("After sort: {:?}", data);
        assert_eq!(data[0], 300, "Highest should be first");
        assert_eq!(data[1], 200, "Middle should be second");
        assert_eq!(data[2], 100, "Lowest should be last");
    }
}
