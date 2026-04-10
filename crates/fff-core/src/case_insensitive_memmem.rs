//! SIMD-accelerated case-insensitive substring search.
//!
//! Implementations (fastest → simplest):
//! - `search_packed_pair`: AVX2 packed-pair scan (two rare bytes at known offsets)
//! - `search`:             memchr2 first-byte scan + verify
//!
//! The packed-pair approach mirrors what `memchr::memmem` does internally for
//! case-sensitive search — pick two rare bytes from the needle, SIMD-scan for
//! both simultaneously, verify candidates.  This gives quadratic selectivity
//! over the single-byte memchr2 approach.

// this is stolen from the memchr2 crate
const BYTE_FREQUENCIES: [u8; 256] = [
    55, 52, 51, 50, 49, 48, 47, 46, 45, 103, 242, 66, 67, 229, 44, 43, // 0x00
    42, 41, 40, 39, 38, 37, 36, 35, 34, 33, 56, 32, 31, 30, 29, 28, // 0x10
    255, 148, 164, 149, 136, 160, 155, 173, 221, 222, 134, 122, 232, 202, 215, 224, // 0x20
    208, 220, 204, 187, 183, 179, 177, 168, 178, 200, 226, 195, 154, 184, 174, 126, // 0x30
    120, 191, 157, 194, 170, 189, 162, 161, 150, 193, 142, 137, 171, 176, 185,
    167, // 0x40 A-O
    186, 112, 175, 192, 188, 156, 140, 143, 123, 133, 128, 147, 138, 146, 114,
    223, // 0x50 P-_
    151, 249, 216, 238, 236, 253, 227, 218, 230, 247, 135, 180, 241, 233, 246,
    244, // 0x60 a-o
    231, 139, 245, 243, 251, 235, 201, 196, 240, 214, 152, 182, 205, 181, 127,
    27, // 0x70 p-DEL
    212, 211, 210, 213, 228, 197, 169, 159, 131, 172, 105, 80, 98, 96, 97, 81, // 0x80
    207, 145, 116, 115, 144, 130, 153, 121, 107, 132, 109, 110, 124, 111, 82, 108, // 0x90
    118, 141, 113, 129, 119, 125, 165, 117, 92, 106, 83, 72, 99, 93, 65, 79, // 0xa0
    166, 237, 163, 199, 190, 225, 209, 203, 198, 217, 219, 206, 234, 248, 158, 239, // 0xb0
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, // 0xc0
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, // 0xd0
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, // 0xe0
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, // 0xf0
];

#[inline]
fn ascii_fold_byte(b: u8) -> u8 {
    if b.is_ascii_uppercase() { b | 0x20 } else { b }
}

/// Toggle ASCII letter case by flipping bit 5.
/// `'n' → 'N'`, `'N' → 'n'`.
#[inline]
fn ascii_swap_case(b: u8) -> u8 {
    b ^ 0x20
}

/// Effective frequency rank for a case-insensitive byte position.
/// Takes the max of lower/upper ranks because we must scan for both.
#[inline]
fn case_insensitive_rank(lower: u8) -> u8 {
    if lower.is_ascii_lowercase() {
        let upper = ascii_swap_case(lower);
        BYTE_FREQUENCIES[lower as usize].max(BYTE_FREQUENCIES[upper as usize])
    } else {
        BYTE_FREQUENCIES[lower as usize]
    }
}

/// Pick two needle positions with the rarest bytes (case-insensitive).
/// Returns (index1, index2) where index1 <= index2.
fn select_rare_pair(needle_lower: &[u8]) -> (usize, usize) {
    debug_assert!(needle_lower.len() >= 2);

    let mut best1 = (u8::MAX, 0usize); // (rank, position)
    let mut best2 = (u8::MAX, 1usize);

    for (i, &b) in needle_lower.iter().enumerate() {
        let r = case_insensitive_rank(b);
        if r < best1.0 {
            best2 = best1;
            best1 = (r, i);
        } else if r < best2.0 && i != best1.1 {
            best2 = (r, i);
        }
    }

    let i1 = best1.1.min(best2.1);
    let i2 = best1.1.max(best2.1);
    (i1, i2)
}

#[inline]
fn verify_scalar(h: *const u8, needle_lower: &[u8]) -> bool {
    for (i, _) in needle_lower.iter().enumerate() {
        if ascii_fold_byte(unsafe { *h.add(i) }) != needle_lower[i] {
            return false;
        }
    }
    true
}

/// AVX2 case-insensitive verify: checks whether `needle_lower` matches
/// the haystack bytes starting at `h`, treating ASCII uppercase as lowercase.
///
/// Processes 32 bytes at a time using a SIMD trick: AVX2 only has a
/// **signed** byte compare (`cmpgt`), but we need an **unsigned** range
/// check (`'A' <= byte <= 'Z'`).  The trick is to XOR every byte with
/// `0x80`, which maps the unsigned range `[0, 255]` into the signed range
/// `[-128, 127]` while preserving order.  After the flip, signed `cmpgt`
/// gives correct unsigned comparisons.
///
/// Once we know which bytes are uppercase, we set bit 5 (`0x20`) on them
/// — this converts `'A'..'Z'` to `'a'..'z'` — then compare against the
/// pre-lowered needle.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn verify_avx2(h: *const u8, needle_lower: &[u8]) -> bool {
    use core::arch::x86_64::*;

    let len = needle_lower.len();
    let mut i = 0usize;

    // Broadcast constants used every iteration:
    //
    //   flip      = 0x80 in every lane — XOR converts unsigned→signed domain
    //   a_minus_1 = ('A' - 1) ^ 0x80  — lower bound for the range check (signed)
    //   z_plus_1  = ('Z' + 1) ^ 0x80  — upper bound for the range check (signed)
    //   bit20     = 0x20 in every lane — OR this onto uppercase bytes to lowercase them
    let flip = _mm256_set1_epi8(0x80u8 as i8);
    let a_minus_1 = _mm256_set1_epi8((b'A' - 1) as i8 ^ 0x80u8 as i8);
    let z_plus_1 = _mm256_set1_epi8((b'Z' + 1) as i8 ^ 0x80u8 as i8);
    let bit20 = _mm256_set1_epi8(0x20u8 as i8);

    while i + 32 <= len {
        // Load 32 bytes from the haystack candidate position.
        let hv = unsafe { _mm256_loadu_si256(h.add(i) as *const __m256i) };
        // Load 32 bytes from the pre-lowercased needle.
        let nv = unsafe { _mm256_loadu_si256(needle_lower.as_ptr().add(i) as *const __m256i) };

        // Flip into signed domain: x = hv ^ 0x80.
        // After this, unsigned ordering is preserved under signed compare.
        let x = _mm256_xor_si256(hv, flip);

        // ge_a[lane] = 0xFF if x[lane] > a_minus_1, i.e. hv[lane] >= 'A' (unsigned).
        let ge_a = _mm256_cmpgt_epi8(x, a_minus_1);
        // le_z[lane] = 0xFF if z_plus_1 > x[lane], i.e. hv[lane] <= 'Z' (unsigned).
        let le_z = _mm256_cmpgt_epi8(z_plus_1, x);
        // upper[lane] = 0xFF only for bytes in the range 'A'..='Z'.
        let upper = _mm256_and_si256(ge_a, le_z);

        // Case-fold: set bit 5 on uppercase bytes → converts 'A'..'Z' to 'a'..'z'.
        // Non-letter bytes are untouched because their `upper` lane is 0x00.
        let folded = _mm256_or_si256(hv, _mm256_and_si256(upper, bit20));

        // Compare the folded haystack against the lowercase needle.
        let eq = _mm256_cmpeq_epi8(folded, nv);
        // movemask extracts the high bit of each lane into a 32-bit mask.
        // All-equal → all high bits set → mask == 0xFFFFFFFF == -1i32.
        if _mm256_movemask_epi8(eq) != -1i32 {
            return false;
        }

        i += 32;
    }

    // Scalar tail: handle remaining bytes that don't fill a full 32-byte vector.
    while i < len {
        if ascii_fold_byte(unsafe { *h.add(i) }) != needle_lower[i] {
            return false;
        }
        i += 1;
    }
    true
}

// ======== NEON + dotprod (aarch64) ===========================================

/// Extract a 16-bit bitmask from a NEON comparison result (each byte 0x00 or 0xFF).
/// Bit *i* of the result corresponds to byte *i* of the input vector.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn neon_movemask(v: core::arch::aarch64::uint8x16_t) -> u16 {
    use core::arch::aarch64::*;

    // AND each byte with its bit-position mask, then horizontally sum each half.
    // Max possible sum per half = 1+2+4+8+16+32+64+128 = 255, fits in u8.
    static BITS: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];
    let bit_mask = unsafe { vld1q_u8(BITS.as_ptr()) };
    let masked = vandq_u8(v, bit_mask);
    let lo = vaddv_u8(vget_low_u8(masked));
    let hi = vaddv_u8(vget_high_u8(masked));
    (lo as u16) | ((hi as u16) << 8)
}

/// NEON + dotprod case-insensitive verify.
///
/// Uses unsigned range checks (NEON has `vcge`/`vcle` for unsigned bytes
/// no XOR-0x80 trick needed unlike AVX2) to detect uppercase ASCII, folds
/// to lowercase, then checks equality via UDOT: XOR the folded haystack
/// with the pre-lowered needle and dot-product the difference with itself.
/// Any non-zero byte produces a non-zero u32 lane.
///
/// The UDOT instruction is emitted via inline asm because the `vdotq_u32`
/// intrinsic is still behind an unstable feature gate on stable Rust.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,dotprod")]
unsafe fn verify_neon_dotprod(h: *const u8, needle_lower: &[u8]) -> bool {
    use core::arch::aarch64::*;

    let len = needle_lower.len();
    let mut i = 0usize;

    let a_val = vdupq_n_u8(b'A');
    let z_val = vdupq_n_u8(b'Z');
    let bit20 = vdupq_n_u8(0x20);

    while i + 16 <= len {
        let hv = unsafe { vld1q_u8(h.add(i)) };
        let nv = unsafe { vld1q_u8(needle_lower.as_ptr().add(i)) };

        // Unsigned range check: 'A' <= byte <= 'Z'
        let upper = vandq_u8(vcgeq_u8(hv, a_val), vcleq_u8(hv, z_val));
        // Case-fold: set bit 5 on uppercase bytes → 'A'..'Z' → 'a'..'z'
        let folded = vorrq_u8(hv, vandq_u8(upper, bit20));

        // XOR with needle — all-zero iff every byte matches.
        let xored = veorq_u8(folded, nv);

        // UDOT: dot(xored, xored) sums squares of 4 consecutive byte
        // differences into each of the 4 u32 lanes (accumulates into zero).
        // Any non-zero byte produces a positive u32 contribution.
        let dots: uint32x4_t;
        let zero = vdupq_n_u32(0);
        unsafe {
            core::arch::asm!(
                "udot {d:v}.4s, {a:v}.16b, {b:v}.16b",
                d = inlateout(vreg) zero => dots,
                a = in(vreg) xored,
                b = in(vreg) xored,
            );
        }

        if vmaxvq_u32(dots) != 0 {
            return false;
        }

        i += 16;
    }

    // Scalar tail
    while i < len {
        if ascii_fold_byte(unsafe { *h.add(i) }) != needle_lower[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// NEON packed-pair kernel: scan 16 haystack positions per iteration,
/// checking two rare bytes (case-insensitive) simultaneously.
/// Same algorithm as the AVX2 version but with 128-bit vectors.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn search_packed_pair_neon(
    haystack: &[u8],
    needle_lower: &[u8],
    i1: usize,
    i2: usize,
) -> bool {
    use core::arch::aarch64::*;

    let n = needle_lower.len();
    let hlen = haystack.len();
    let ptr = haystack.as_ptr();
    let last_start = hlen - n;

    let b1 = needle_lower[i1];
    let b1_alt = if b1.is_ascii_lowercase() {
        ascii_swap_case(b1)
    } else {
        b1
    };
    let b2 = needle_lower[i2];
    let b2_alt = if b2.is_ascii_lowercase() {
        ascii_swap_case(b2)
    } else {
        b2
    };

    let v1_lo = vdupq_n_u8(b1);
    let v1_hi = vdupq_n_u8(b1_alt);
    let v2_lo = vdupq_n_u8(b2);
    let v2_hi = vdupq_n_u8(b2_alt);

    let max_idx = i1.max(i2);
    let max_offset = hlen.saturating_sub(max_idx + 16);
    let mut offset = 0usize;

    while offset <= max_offset {
        let chunk1 = unsafe { vld1q_u8(ptr.add(offset + i1)) };
        let chunk2 = unsafe { vld1q_u8(ptr.add(offset + i2)) };

        // Case-insensitive match: OR both case variants, then AND the two positions.
        let eq1 = vorrq_u8(vceqq_u8(chunk1, v1_lo), vceqq_u8(chunk1, v1_hi));
        let eq2 = vorrq_u8(vceqq_u8(chunk2, v2_lo), vceqq_u8(chunk2, v2_hi));

        let mut mask = unsafe { neon_movemask(vandq_u8(eq1, eq2)) };

        while mask != 0 {
            let bit = mask.trailing_zeros() as usize;
            let candidate = offset + bit;
            if candidate > last_start {
                return false;
            }
            if unsafe { verify_dispatch(ptr.add(candidate), needle_lower) } {
                return true;
            }
            mask &= mask - 1;
        }

        offset += 16;
    }

    // Tail: remaining positions that couldn't fill a full vector.
    if offset <= last_start {
        let rare_pos =
            if case_insensitive_rank(needle_lower[i1]) <= case_insensitive_rank(needle_lower[i2]) {
                i1
            } else {
                i2
            };
        let rare_byte = needle_lower[rare_pos];
        let tail_start = offset + rare_pos;
        let tail_end = last_start + rare_pos + 1;
        if tail_start < tail_end {
            let tail_space = &haystack[tail_start..tail_end];
            if rare_byte.is_ascii_lowercase() {
                for pos in memchr::memchr2_iter(rare_byte, ascii_swap_case(rare_byte), tail_space) {
                    let candidate = offset + pos;
                    if unsafe { verify_dispatch(ptr.add(candidate), needle_lower) } {
                        return true;
                    }
                }
            } else {
                for pos in memchr::memchr_iter(rare_byte, tail_space) {
                    let candidate = offset + pos;
                    if unsafe { verify_dispatch(ptr.add(candidate), needle_lower) } {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[inline]
unsafe fn verify_dispatch(h: *const u8, needle_lower: &[u8]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if needle_lower.len() >= 32 && std::is_x86_feature_detected!("avx2") {
            return unsafe { verify_avx2(h, needle_lower) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if needle_lower.len() >= 16 && std::arch::is_aarch64_feature_detected!("dotprod") {
            return unsafe { verify_neon_dotprod(h, needle_lower) };
        }
    }

    verify_scalar(h, needle_lower)
}

// ── Packed-pair search (AVX2) ───────────────────────────────────────────

/// AVX2 packed-pair kernel: scan 32 haystack positions per iteration,
/// checking two rare bytes (case-insensitive) simultaneously.
/// 4 cmpeq + 2 or + 1 and + 1 movemask per 32 bytes — same memory
/// bandwidth as memchr2 but quadratic selectivity.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn search_packed_pair_avx2(
    haystack: &[u8],
    needle_lower: &[u8],
    i1: usize,
    i2: usize,
) -> bool {
    use core::arch::x86_64::*;

    let n = needle_lower.len();
    let hlen = haystack.len();
    let ptr = haystack.as_ptr();
    let last_start = hlen - n; // last valid match-start position

    let b1 = needle_lower[i1];
    let b1_alt = if b1.is_ascii_lowercase() {
        ascii_swap_case(b1)
    } else {
        b1
    };
    let b2 = needle_lower[i2];
    let b2_alt = if b2.is_ascii_lowercase() {
        ascii_swap_case(b2)
    } else {
        b2
    };

    let v1_lo = _mm256_set1_epi8(b1 as i8);
    let v1_hi = _mm256_set1_epi8(b1_alt as i8);
    let v2_lo = _mm256_set1_epi8(b2 as i8);
    let v2_hi = _mm256_set1_epi8(b2_alt as i8);

    // Main loop: process 32 candidate positions per iteration.
    // We load from ptr+offset+i1 and ptr+offset+i2, so we need
    // offset + max(i1,i2) + 31 < hlen.
    let max_idx = i1.max(i2);
    let max_offset = hlen.saturating_sub(max_idx + 32);
    let mut offset = 0usize;

    while offset <= max_offset {
        let chunk1 = unsafe { _mm256_loadu_si256(ptr.add(offset + i1) as *const __m256i) };
        let chunk2 = unsafe { _mm256_loadu_si256(ptr.add(offset + i2) as *const __m256i) };

        // Case-insensitive match: OR both case variants, then AND the two positions.
        let eq1 = _mm256_or_si256(
            _mm256_cmpeq_epi8(chunk1, v1_lo),
            _mm256_cmpeq_epi8(chunk1, v1_hi),
        );
        let eq2 = _mm256_or_si256(
            _mm256_cmpeq_epi8(chunk2, v2_lo),
            _mm256_cmpeq_epi8(chunk2, v2_hi),
        );

        let mut mask = _mm256_movemask_epi8(_mm256_and_si256(eq1, eq2)) as u32;

        while mask != 0 {
            let bit = mask.trailing_zeros() as usize;
            let candidate = offset + bit;
            if candidate > last_start {
                // Past the end — no more valid positions in this or future chunks.
                return false;
            }
            if unsafe { verify_dispatch(ptr.add(candidate), needle_lower) } {
                return true;
            }
            mask &= mask - 1;
        }

        offset += 32;
    }

    // Tail: remaining positions that couldn't fill a full vector.
    // Use memchr2 on the rarest byte for these last few positions.
    if offset <= last_start {
        let rare_pos =
            if case_insensitive_rank(needle_lower[i1]) <= case_insensitive_rank(needle_lower[i2]) {
                i1
            } else {
                i2
            };
        let rare_byte = needle_lower[rare_pos];
        let tail_start = offset + rare_pos;
        let tail_end = last_start + rare_pos + 1;
        if tail_start < tail_end {
            let tail_space = &haystack[tail_start..tail_end];
            if rare_byte.is_ascii_lowercase() {
                for pos in memchr::memchr2_iter(rare_byte, ascii_swap_case(rare_byte), tail_space) {
                    let candidate = offset + pos;
                    if unsafe { verify_dispatch(ptr.add(candidate), needle_lower) } {
                        return true;
                    }
                }
            } else {
                for pos in memchr::memchr_iter(rare_byte, tail_space) {
                    let candidate = offset + pos;
                    if unsafe { verify_dispatch(ptr.add(candidate), needle_lower) } {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Packed-pair case-insensitive substring search.
///
/// Selects the two rarest bytes from the needle (using the memchr byte
/// frequency heuristic), then SIMD-scans for both at their known offsets
/// simultaneously.  Falls back to `search` for needles shorter than 2 bytes.
pub fn search_packed_pair(haystack: &[u8], needle_lower: &[u8]) -> bool {
    let n = needle_lower.len();
    if n == 0 {
        return true;
    }
    if n < 2 {
        return search(haystack, needle_lower);
    }
    if n > haystack.len() {
        return false;
    }

    let (i1, i2) = select_rare_pair(needle_lower);

    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // Need enough haystack for at least one vector load.
            let max_idx = i1.max(i2);
            if haystack.len() >= max_idx + 32 {
                return unsafe { search_packed_pair_avx2(haystack, needle_lower, i1, i2) };
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // The NEON packed-pair scan checks 16 bytes/iteration with ~7 ops,
        // while memchr's optimized loop processes more bytes with fewer ops.
        // Packed-pair wins when the first byte is common (lots of false
        // positives for memchr2 that we avoid). But when the first byte is
        // rare (z, q, x, ...) memchr2 has no false positives and its raw
        // throughput dominates. Threshold 200 on the frequency table splits
        // common letters (s=243, e=253, f=227) from rare ones (z=152, q=139).
        let first_byte_rank = case_insensitive_rank(needle_lower[0]);
        let max_idx = i1.max(i2);
        if first_byte_rank >= 200 && haystack.len() >= max_idx + 16 {
            return unsafe { search_packed_pair_neon(haystack, needle_lower, i1, i2) };
        }
    }

    // Fallback for short haystacks or non-SIMD platforms.
    search(haystack, needle_lower)
}

// ── Original memchr2 first-byte search ──────────────────────────────────

/// Case-insensitive search using memchr2 on the first byte.
pub fn search(haystack: &[u8], needle_lower: &[u8]) -> bool {
    let n = needle_lower.len();
    if n == 0 {
        return true;
    }
    if n > haystack.len() {
        return false;
    }

    let search_space = &haystack[..=haystack.len() - n];
    let first = needle_lower[0];

    if first.is_ascii_lowercase() {
        let alt = ascii_swap_case(first);
        for pos in memchr::memchr2_iter(first, alt, search_space) {
            if unsafe { verify_dispatch(haystack.as_ptr().add(pos), needle_lower) } {
                return true;
            }
        }
    } else {
        for pos in memchr::memchr_iter(first, search_space) {
            if unsafe { verify_dispatch(haystack.as_ptr().add(pos), needle_lower) } {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_case_insensitive() {
        assert!(search_packed_pair(b"Hello World", b"hello"));
        assert!(search_packed_pair(b"Hello World", b"world"));
        assert!(search_packed_pair(b"NOMORE bugs", b"nomore"));
        assert!(!search_packed_pair(b"Hello World", b"xyz"));
    }

    #[test]
    fn edge_cases() {
        assert!(search_packed_pair(b"ab", b"ab"));
        assert!(search_packed_pair(b"AB", b"ab"));
        assert!(!search_packed_pair(b"a", b"ab"));
        assert!(search_packed_pair(b"anything", b""));
        assert!(!search_packed_pair(b"", b"x"));
    }

    #[test]
    fn packed_pair_matches_search() {
        let haystacks: &[&[u8]] = &[
            b"The quick brown fox jumps over the lazy dog",
            b"int mutex_lock(struct mutex *lock) { return 0; }",
            b"#define NOMORE_RETRIES 5\nif (nomore) return;",
            b"abcdefghijklmnopqrstuvwxyz",
            b"short",
        ];
        let needles: &[&[u8]] = &[b"fox", b"mutex", b"nomore", b"xyz", b"the", b"short", b"qr"];
        for h in haystacks {
            for n in needles {
                let lower: Vec<u8> = n.iter().map(|b| b.to_ascii_lowercase()).collect();
                assert_eq!(
                    search_packed_pair(h, &lower),
                    search(h, &lower),
                    "mismatch for haystack={:?} needle={:?}",
                    std::str::from_utf8(h),
                    std::str::from_utf8(n),
                );
            }
        }
    }

    #[test]
    fn long_haystack_neon_path() {
        // Haystack > 16 bytes exercises NEON packed-pair search loop
        let haystack =
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaTHIS_IS_A_LONG_NEEDLE_TESTbbbbbbbbbbbbbbbbbb";
        assert!(search_packed_pair(haystack, b"this_is_a_long_needle_test"));
        assert!(!search_packed_pair(
            haystack,
            b"this_is_a_long_needle_testz"
        ));

        // Needle >= 16 bytes exercises NEON dotprod verify
        let long_needle = b"struct mutex *lock";
        let haystack2 = b"int STRUCT MUTEX *LOCK(struct mutex *lock) { return 0; }";
        assert!(search_packed_pair(haystack2, long_needle));

        // All uppercase haystack, lowercase needle
        let upper_hay = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        assert!(search_packed_pair(upper_hay, b"qrstuvwxyz0123456789a"));
        assert!(!search_packed_pair(upper_hay, b"qrstuvwxyz01234567899"));

        // Needle at very end
        let end_hay = b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxfind_me";
        assert!(search_packed_pair(end_hay, b"find_me"));

        // Needle at very start
        assert!(search_packed_pair(end_hay, b"xx"));

        // 1KB haystack with needle near the end
        let mut big = vec![b'z'; 1024];
        big[1000..1010].copy_from_slice(b"hElLo_WoRl");
        assert!(search_packed_pair(&big, b"hello_wo"));
        assert!(!search_packed_pair(&big, b"hello_world"));
    }

    #[test]
    fn rare_pair_selection() {
        // For "nomore": n=246, o=244, m=233, o=244, r=245, e=253
        // Rarest positions should include 'm' (pos 2, rank 233)
        let (i1, i2) = select_rare_pair(b"nomore");
        let ranks: Vec<u8> = b"nomore"
            .iter()
            .map(|&b| case_insensitive_rank(b))
            .collect();
        let r1 = ranks[i1];
        let r2 = ranks[i2];
        // Both selected ranks should be <= all other ranks
        for (i, &r) in ranks.iter().enumerate() {
            if i != i1 && i != i2 {
                assert!(r1 <= r || r2 <= r, "pair ({i1},{i2}) not optimal");
            }
        }
    }
}
