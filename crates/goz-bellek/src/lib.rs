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

/// Bounded best-`k` collection: once `v` holds `2k` items, keeps only the `k`
/// smallest under `cmp` (quickselect, order within them unspecified). Called
/// after every push, it caps a scan's memory at `2k` items while the final `k`
/// stays exactly the best `k` of everything pushed. `k == 0` is a no-op.
pub fn keep_best<T>(v: &mut Vec<T>, k: usize, cmp: impl FnMut(&T, &T) -> std::cmp::Ordering) {
    if k > 0 && v.len() >= 2 * k {
        v.select_nth_unstable_by(k - 1, cmp);
        v.truncate(k);
    }
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
    fn keep_best_bounds_memory_and_keeps_the_best() {
        let mut v = Vec::new();
        for x in (0..1000u32).rev() {
            v.push((x * 7919) % 1000);
            keep_best(&mut v, 5, |a, b| a.cmp(b));
            assert!(v.len() < 10);
        }
        keep_best(&mut v, 5, |a, b| a.cmp(b)); // may still hold up to 2k-1
        v.sort_unstable();
        assert_eq!(&v[..5], [0, 1, 2, 3, 4]);
        let mut w = vec![3, 1];
        keep_best(&mut w, 0, |a: &i32, b| a.cmp(b));
        assert_eq!(w, [3, 1], "k = 0 leaves the Vec alone");
    }

    #[test]
    fn grow_step_is_a_sixteenth_with_floor() {
        assert_eq!(grow_step(10), 1 << 16);
        assert_eq!(grow_step(6_400_000), 400_000);
    }
}
