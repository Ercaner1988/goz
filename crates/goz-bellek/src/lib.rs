//! Allocation helpers for the struct-of-arrays column stores in `goz-core`.
//!
//! Split out of `goz-core` (which is over the 1500-line crate limit and may
//! not grow). Two problems they solve, both measured on a 6.17 M-entry C:
//! volume (`goz --status`, see tikel-gozlemci `docs/ARAYUZ-PLANI.md` §6):
//!
//! * `Vec::shrink_to` is a no-op under mimalloc: `mi_realloc_aligned` keeps the
//!   block whenever the new size is at least half the old one, and a doubled
//!   Vec is always above half. [`shrink_vec`] copies into a fresh allocation.
//! * Doubling growth after the post-bootstrap shrink re-doubles every column on
//!   live churn (entries: 664 MB allocated for 354 MB used after 18 h).
//!   [`grow_step`] grows by a sixteenth instead.

/// Shrinks a Vec to its length plus ~1.6% headroom, really returning the slack
/// (copy into a fresh allocation; transient peak is this one column). Never
/// exact-fit: the next append would re-grow the whole column.
pub fn shrink_vec<T>(v: &mut Vec<T>) {
    let target = v.len() + v.len() / 64 + 64;
    if v.capacity() <= target {
        return;
    }
    let mut fresh = Vec::with_capacity(target);
    fresh.append(v);
    *v = fresh;
}

/// Growth step for append-only columns: a sixteenth of the length, at least
/// 64 Ki elements (≤ ~6% slack instead of up to 50%).
pub fn grow_step(len: usize) -> usize {
    (len / 16).max(1 << 16)
}

/// Reserves exactly `additional` more elements (used through [`each_column!`]).
pub fn reserve<T>(v: &mut Vec<T>, additional: usize) {
    v.reserve_exact(additional);
}

/// Applies a generic function to every column of `goz-core`'s `EntryTable`
/// (`$f(&mut table.column, args…)`), so the 12-column lists exist once.
#[macro_export]
macro_rules! each_column {
    ($t:expr, $f:path $(, $a:expr)*) => {{
        let t = &mut *$t;
        $f(&mut t.frn $(, $a)*);
        $f(&mut t.parent $(, $a)*);
        $f(&mut t.name_id $(, $a)*);
        $f(&mut t.flags $(, $a)*);
        $f(&mut t.size $(, $a)*);
        $f(&mut t.mtime $(, $a)*);
        $f(&mut t.next_link $(, $a)*);
        $f(&mut t.next_same $(, $a)*);
        $f(&mut t.prev_same $(, $a)*);
        $f(&mut t.first_child $(, $a)*);
        $f(&mut t.next_child $(, $a)*);
        $f(&mut t.prev_child $(, $a)*);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_vec_really_reallocates_and_keeps_data() {
        let mut v: Vec<u32> = Vec::with_capacity(1 << 20);
        v.extend(0..600_000u32);
        shrink_vec(&mut v);
        assert!(
            v.capacity() <= 600_000 + 600_000 / 64 + 64,
            "{}",
            v.capacity()
        );
        assert_eq!((v.len(), v[599_999]), (600_000, 599_999));
    }

    #[test]
    fn grow_step_is_a_sixteenth_with_floor() {
        assert_eq!(grow_step(10), 1 << 16);
        assert_eq!(grow_step(6_400_000), 400_000);
    }
}
