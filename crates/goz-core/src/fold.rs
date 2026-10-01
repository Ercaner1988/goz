//! Case folding for case-insensitive matching.
//!
//! Two tiers: an ASCII fast path (`A..=Z` → `+0x20`) and, for non-ASCII,
//! canonical decomposition, a simple 1:1 Unicode lowercase of each scalar (the
//! first scalar of `char::to_lowercase`), then canonical RECOMPOSITION. NFD makes the same text fold
//! identically whether it arrived precomposed (NFC, `é` = U+00E9) or decomposed
//! (NFD, `e` + U+0301): a filename copied from macOS matches a query
//! typed on Windows. Combining marks are KEPT, so unifying the normalization
//! form never merges genuinely distinct letters (Cyrillic `й` stays distinct
//! from `и`); accent-insensitivity is deliberately NOT done here. The result is
//! close to, but not identical to, NTFS's own `$UpCase` comparison: `$UpCase` is
//! a per-code-unit table and does not normalize, so goz additionally treats NFC
//! and NFD spellings of the same text as equal (any precomposed letter with a
//! canonical decomposition). Matching never affects index integrity. Both the
//! needle and each candidate name are folded with the same function, so
//! comparisons stay consistent.
//!
//! Input and output are WTF-8. Unpaired surrogates (code points `U+D800..=
//! U+DFFF`, which a real text needle can never contain) pass through
//! unchanged.

use crate::wtf8::CodePoints;
use smallvec::SmallVec;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::decompose_canonical;

/// Folds `bytes` (WTF-8) into `out` (WTF-8), case-insensitively.
pub fn fold_into(bytes: &[u8], out: &mut Vec<u8>) {
    // ASCII fast path: no canonical decomposition or combining marks are
    // possible below U+0080, so fold byte-wise with no decode.
    if bytes.is_ascii() {
        out.reserve(bytes.len());
        for &b in bytes {
            out.push(b.to_ascii_lowercase());
        }
        return;
    }
    fold_non_ascii(bytes, out);
}

/// The non-ASCII path, deliberately OUT of line.
///
/// It needs a `SmallVec<[char; 256]>` scratch, which is a 1 KiB stack frame, and
/// Rust reserves a function's whole frame on entry. Inlined into `fold_into`
/// that 1 KiB was paid by the ASCII fast path too, on every call, including the
/// per-candidate extension folding on the query hot path. Splitting it out keeps
/// `fold_into` small enough to inline and leaves the big frame to the rare path
/// that actually needs it.
#[inline(never)]
fn fold_non_ascii(bytes: &[u8], out: &mut Vec<u8>) {
    // Decompose (unify normalization form), simple-lowercase each scalar, then
    // RECOMPOSE. The round trip is what makes NFC and NFD spellings equal
    // without also making `é` equal to `e`: emitting the decomposed form would
    // leave "café" as "cafe" + U+0301, which a substring search for "cafe"
    // matches on the prefix, and which counts as five code points so `????`
    // stops matching a four-character name.
    //
    // Composition spans code points (an NFD name arrives as `e` then U+0301 as
    // two separate ones), so this buffers a run rather than working per input
    // scalar. The run is stack-sized for any NTFS name; only the non-ASCII path
    // pays for it at all.
    let mut run: SmallVec<[char; 256]> = SmallVec::new();
    for cp in CodePoints::new(bytes) {
        match char::from_u32(cp) {
            // `İ` decomposes to `I` + U+0307; the dot would survive lowering and
            // "İSTANBUL" would never match "istanbul".
            Some('İ') => run.push('i'),
            Some(c) => decompose_canonical(c, |d| run.push(lower_char(d))),
            // Surrogate / non-scalar (WTF-8): it can take part in no
            // composition, so flush the run and pass it through unchanged.
            None => {
                compose_into(&mut run, out);
                encode(cp, out);
            }
        }
    }
    compose_into(&mut run, out);
}

/// Canonically composes `run` into `out` as WTF-8 and clears it.
fn compose_into(run: &mut SmallVec<[char; 256]>, out: &mut Vec<u8>) {
    // An empty run composes to nothing; no early return needed.
    for c in run.iter().copied().nfc() {
        encode(c as u32, out);
    }
    run.clear();
}

pub fn fold(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    fold_into(bytes, &mut out);
    out
}

/// Simple 1:1 lowercase of one scalar: the first scalar of the full lowercase
/// mapping. 1:1 and stable, unlike full folding (ß→ss) which is not
/// length-preserving and diverges from filesystem semantics.
///
/// Turkish dotless `ı` also folds to `i`: `I` already lowers to `i` and `İ` to
/// `i` + U+0307 (first scalar `i`), so without this "IŞIK" never finds
/// "ışık.txt". All four i-letters meet at `i`; accents are still kept.
/// (`to_lowercase` always yields a scalar, so the fallback is only ever `ı`.)
fn lower_char(c: char) -> char {
    c.to_lowercase().next().filter(|&l| l != 'ı').unwrap_or('i')
}

/// Encodes one code point as WTF-8 (surrogates permitted).
fn encode(cp: u32, out: &mut Vec<u8>) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp < 0x800 {
        out.push(0xC0 | (cp >> 6) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    } else if cp < 0x10000 {
        out.push(0xE0 | (cp >> 12) as u8);
        out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    } else {
        out.push(0xF0 | (cp >> 18) as u8);
        out.push(0x80 | ((cp >> 12) & 0x3F) as u8);
        out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_folds_case() {
        assert_eq!(fold(b"Report.PDF"), b"report.pdf");
        assert_eq!(fold(b"already lower"), b"already lower");
        assert_eq!(fold(b"MiXeD123"), b"mixed123");
    }

    #[test]
    fn fold_is_idempotent() {
        for s in ["Foo", "ÇAĞAÇ", "Straße", "ФайЛ"] {
            let once = fold(s.as_bytes());
            let twice = fold(&once);
            assert_eq!(once, twice, "fold not idempotent for {s}");
        }
    }

    /// Turkish i-letters meet at `i` whichever case the name or the query uses.
    #[test]
    fn turkish_i_letters_fold_together() {
        assert_eq!(fold("IŞIK".as_bytes()), fold("ışık".as_bytes()));
        assert_eq!(fold("İSTANBUL".as_bytes()), fold(b"istanbul"));
        assert_eq!(fold("ılık".as_bytes()), "ilik".as_bytes());
        assert_ne!(
            fold("çay".as_bytes()),
            fold(b"cay"),
            "accents are not folded"
        );
    }

    #[test]
    fn needle_and_haystack_fold_consistently() {
        // A folded needle is found in a folded haystack regardless of the
        // original case of either.
        let hay = fold("MyÇAĞAÇFile".as_bytes());
        let needle = fold("çağaç".as_bytes());
        assert!(
            hay.windows(needle.len()).any(|w| w == needle.as_slice()),
            "case-insensitive substring should match across cases"
        );
    }

    #[test]
    fn non_ascii_lowercases() {
        // Turkish/Cyrillic uppercase → lowercase (via NFD + simple lowercase).
        assert!(!fold("İ".as_bytes()).is_empty());
        // Case-insensitive Cyrillic: upper and lower fold to the same thing.
        assert_eq!(fold("ФАЙЛ".as_bytes()), fold("файл".as_bytes()));
    }

    #[test]
    fn nfc_and_nfd_fold_identically() {
        // "café": precomposed (NFC, é = U+00E9) vs decomposed (NFD, e + U+0301)
        // must fold to the same bytes so a query matches regardless of the
        // normalization form the filename was stored in.
        let nfc = "caf\u{00E9}"; // café, precomposed
        let nfd = "cafe\u{0301}"; // café, e + combining acute
        assert_ne!(nfc.as_bytes(), nfd.as_bytes(), "inputs differ byte-wise");
        assert_eq!(fold(nfc.as_bytes()), fold(nfd.as_bytes()));
        // Combining marks are kept (not accent-stripped): "cafe" ≠ "café".
        // Byte inequality is too weak on its own: see
        // folding_is_not_accent_insensitive for the invariant that matters.
        assert_ne!(fold("cafe".as_bytes()), fold(nfc.as_bytes()));
    }

    /// Unifying the normalization form must not smuggle in accent-insensitivity.
    ///
    /// Byte inequality was too weak to catch that: emitting the DECOMPOSED form
    /// left the folded name as "cafe" + U+0301, which is unequal to "cafe" while
    /// still CONTAINING it, so every substring query for "cafe" matched it. The
    /// invariant that matters is that the folded needle does not OCCUR in the
    /// folded haystack.
    #[test]
    fn folding_is_not_accent_insensitive() {
        let needle = fold("cafe".as_bytes());
        for spelling in ["caf\u{00E9}", "cafe\u{0301}"] {
            let hay = fold(spelling.as_bytes());
            assert!(
                !hay.windows(needle.len()).any(|w| w == needle.as_slice()),
                "{spelling:?}: a search for cafe must not match an accented name"
            );
        }
        // The same rule from the other side: a genuinely distinct letter stays
        // distinct through the decompose/recompose round trip. Cyrillic U+0439
        // decomposes to U+0438 + U+0306.
        assert_ne!(fold("\u{0439}".as_bytes()), fold("\u{0438}".as_bytes()));
    }

    /// A precomposed character folds to ONE code point, so `?` (which matches
    /// exactly one) keeps counting visible characters. The decomposed form made
    /// a 4-character name 5 code points and quietly broke `????`.
    #[test]
    fn folding_preserves_code_point_arity() {
        for spelling in ["caf\u{00E9}", "cafe\u{0301}"] {
            let folded = fold(spelling.as_bytes());
            assert_eq!(
                crate::wtf8::to_utf16(&folded).len(),
                4,
                "{spelling:?} must fold to 4 code points, not a decomposed 5"
            );
        }
    }

    #[test]
    fn surrogates_pass_through() {
        // Lone high surrogate embedded in WTF-8 survives folding.
        let mut bytes = Vec::new();
        crate::wtf8::from_utf16(&[0x0041, 0xD800, 0x0042], &mut bytes);
        let folded = fold(&bytes);
        // 'A' → 'a', 'B' → 'b', surrogate unchanged.
        assert_eq!(crate::wtf8::to_utf16(&folded), vec![0x0061, 0xD800, 0x0062]);
    }

    use proptest::prelude::*;

    proptest! {
        /// `fold` is total (never panics) and idempotent over arbitrary UTF-16
        /// (unpaired surrogates included): it is the case-fold basis of every
        /// query match, so folding a folded name must be a fixpoint.
        #[test]
        fn fold_is_total_and_idempotent(units in proptest::collection::vec(any::<u16>(), 0..64)) {
            let mut bytes = Vec::new();
            crate::wtf8::from_utf16(&units, &mut bytes);
            let once = fold(&bytes);
            let twice = fold(&once);
            prop_assert_eq!(once, twice);
        }

        /// The decoder path inside `fold` is total on arbitrary bytes.
        #[test]
        fn fold_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
            let _ = fold(&bytes);
        }
    }
}
