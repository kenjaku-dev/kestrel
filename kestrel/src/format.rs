//! Formatting of machine values.
//!
//! §2.8's mono/sans split is a hard rule: sizes, dates, counts, permissions and
//! the status-bar path are `type.meta` in `FontFamily::Monospace`; human names
//! (file names, sidebar labels) are proportional. This module is the mono side of
//! that split.
//!
//! It is deliberately dependency-free: pulling in a date crate for
//! `strftime("%Y-%m-%d %H:%M")` is not a trade this app makes. Everything here
//! is a pure function of its input — no `localtime_r`, no locale, no I/O — so
//! nothing in here can block a frame.

use std::time::{SystemTime, UNIX_EPOCH};

/// The em dash the spec uses for "not applicable".
///
/// §4.2's caveat: `FileEntry::size` for a *directory* is that directory's own
/// `lstat` length (typically 4096), **not** a recursive total. Printing `4.0 KiB`
/// there is a lie, so directories render this instead.
pub const NOT_APPLICABLE: &str = "\u{2014}";

/// Formats a byte count with a **binary** prefix suffix.
///
/// Tabular-figure alignment is the reason for a fixed one-decimal format rather
/// than egui's stock `{}` on a `String`: the decimal point must land in the same
/// column on every row or the column stops being scannable.
///
/// # `KiB`, not `KB`
///
/// The divisor is 1024, so the honest suffix is `KiB`/`MiB`/… — an SI `KB` is
/// 1000 bytes, and labelling a 1024-byte count `1.0 KB` states a number that is
/// 2.4% wrong. IEC names are also what `ls --si` and every modern disk tool
/// prints, so the column matches what the user sees elsewhere. The alternative
/// — switching the divisor to 1000 — was rejected: a file manager's job is to
/// report the size the filesystem actually reports, in the unit the platform
/// measures in, and a 1000-based "size" disagrees with `stat`, `ls -l` and every
/// property dialog on the machine.
#[must_use]
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut size = value as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        // Whole bytes: no decimal point, so the column is narrow and exact.
        format!("{} {}", value, UNITS[0])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Formats a `SystemTime` as `YYYY-MM-DD HH:MM`, in **UTC**.
///
/// §4.2 `row.col-modified` says "absolute UTC shown on hover", so UTC is the
/// right column value: a column that shifts when the user crosses a timezone is
/// a column nobody can read. Converting by hand (rather than through `chrono`)
/// keeps this a pure function with no dependency and no syscall.
#[must_use]
pub fn timestamp(time: SystemTime) -> String {
    let secs = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        // Pre-epoch times are legal on a filesystem; clamp rather than panic.
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let (y, mo, d, h, mi) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

/// Howard Hinnant's `civil_from_days`, plus the time-of-day split.
///
/// Days-to-civil is exact for the whole proleptic Gregorian range and needs no
/// lookup table, which is why it is spelled out rather than approximated.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi) = ((rem / 3600) as u32, ((rem % 3600) / 60) as u32);

    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d, h, mi)
}

/// Middle-truncates `text` to at most `max_cols` columns.
///
/// §2.8: "all list text is single-line and vertically centred in its row (never
/// wrapped, never clamped mid-word — middle-truncate with ellipsis at 60% width
/// if needed)". The *path* is the one column where the interesting part is in
/// the middle, so `.../src/kestrel/src` beats `kestrel/src/tokens...`.
///
/// Char-boundary safe: it walks `char_indices`, so a multi-byte name can never
/// panic on a slice.
#[must_use]
pub fn middle_truncate(text: &str, max_cols: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_cols {
        return text.to_string();
    }
    if max_cols <= 3 {
        // No room for head + ellipsis + tail. A single ellipsis is honest.
        return "\u{2026}".to_string();
    }
    // Split the budget so the result is exactly `max_cols` wide, not
    // `max_cols - 1`: integer division would otherwise leave a column of dead
    // space on every truncated row, which is one pixel of misalignment per row
    // in a 50,000-row list.
    let ellipsis = 1usize;
    let tail = (max_cols - ellipsis) / 2;
    let head = max_cols - ellipsis - tail;
    let head: String = chars[..head].iter().collect();
    let tail: String = chars[chars.len() - tail..].iter().collect();
    format!("{head}\u{2026}{tail}")
}

/// End-truncates `text` to at most `max_cols` columns, with a trailing ellipsis.
///
/// # When this and [`middle_truncate`] are the right answer
///
/// [`middle_truncate`] exists because §2.8 says list text is
/// middle-truncated, and the reason that is right for a *path* or a *file name*
/// is that the identifying part is neither end. It is the wrong answer for
/// prose: a sentence's payload is its ending, and
/// `"System follows the \u{2026}r dark preference."` reads as a corrupted string
/// rather than as an elision. This is the variant for anything read
/// left-to-right as a sentence.
///
/// Char-boundary safe, for the same reason and by the same `chars` walk.
#[must_use]
pub fn end_truncate(text: &str, max_cols: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_cols {
        return text.to_string();
    }
    if max_cols <= 1 {
        return "\u{2026}".to_string();
    }
    let head: String = chars[..max_cols - 1].iter().collect();
    format!("{head}\u{2026}")
}

/// Pluralises `count` against `singular`/`plural`.
///
/// A status bar that says "1 items" is a bug, and this app has enough places to
/// show every count to hit it eventually.
#[must_use]
pub fn plural(count: usize, singular: &'static str, plural: &'static str) -> String {
    let word = if count == 1 { singular } else { plural };
    // Thousands separators: a 50,000-item directory is the case this exists for.
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + word.len() + 1);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.push(' ');
    out.push_str(word);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bytes_scales_through_the_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        // Binary divisors, so the labels are the IEC ones. `1.0 KB` for 1024
        // bytes would be a 2.4% lie in the one column a user checks.
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
        assert_eq!(bytes(1024u64.pow(5)), "1.0 PiB");
        // The largest unit must not be exceeded by an absurd value.
        assert!(bytes(u64::MAX).ends_with(" PiB"));
    }

    #[test]
    fn a_kibibyte_is_exactly_1024_bytes() {
        // The reason the suffix is `KiB`: the divisor and the label agree, so a
        // value can be read back without knowing which convention was used.
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1025), "1.0 KiB");
        assert_eq!(bytes(1024 * 1024 - 1), "1024.0 KiB");
    }

    #[test]
    fn same_unit_values_have_equal_width() {
        // Tabular alignment is the whole point, and the cell is right-aligned, so
        // two values in the same unit must produce strings of the same length —
        // otherwise the decimal point jumps a column.
        assert_eq!(bytes(1024).len(), bytes(9 * 1024).len());
        assert_eq!(bytes(2 * 1024 * 1024).len(), bytes(9 * 1024 * 1024).len());
        // A unit change *does* change the width, which is expected: the unit
        // itself is the alignment anchor.
        assert_ne!(bytes(1024).len(), bytes(999).len());
    }

    #[test]
    fn the_fraction_always_has_exactly_one_decimal() {
        // Two decimals would double the width of every value in a column.
        for n in [1024u64, 1536, 1024 * 1024, 7 * 1024 * 1024 * 1024] {
            let s = bytes(n);
            let frac = s.split('.').nth(1).expect("a decimal point");
            assert_eq!(
                frac.chars().take_while(|c| c.is_ascii_digit()).count(),
                1,
                "{s}"
            );
        }
    }

    #[test]
    fn timestamps_match_known_utc_instants() {
        // 1970-01-01T00:00:00Z
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01 00:00");
        // 2001-09-09T01:46:40Z — the "a meter of GNU" instant.
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(1_000_000_000)),
            "2001-09-09 01:46"
        );
        // 2024-02-29T12:00:00Z — a leap day, which is the day the day-count
        // arithmetic is most likely to be wrong.
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(1_709_208_000)),
            "2024-02-29 12:00"
        );
        // 2000-03-01, the 400-year-era boundary the algorithm special-cases.
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(951_868_800)),
            "2000-03-01 00:00"
        );
    }

    #[test]
    fn pre_epoch_timestamps_do_not_panic() {
        assert_eq!(
            timestamp(UNIX_EPOCH - Duration::from_secs(86_400)),
            "1969-12-31 00:00"
        );
    }

    #[test]
    fn middle_truncate_keeps_both_ends() {
        assert_eq!(middle_truncate("short", 20), "short");
        let long = "/home/achraf/Projects/kestrel/kestrel/src/tokens.rs";
        let t = middle_truncate(long, 20);
        assert_eq!(t.chars().count(), 20);
        assert!(t.starts_with("/home"), "got {t}");
        assert!(t.ends_with("tokens.rs"), "got {t}");
        assert!(t.contains('\u{2026}'));
    }

    #[test]
    fn middle_truncate_degrades_gracefully_on_tiny_widths() {
        assert_eq!(middle_truncate("abcdef", 3), "\u{2026}");
        assert_eq!(middle_truncate("abc", 2), "\u{2026}");
        assert_eq!(middle_truncate("", 10), "");
    }

    #[test]
    fn middle_truncate_never_splits_a_char() {
        // A multi-byte name must not panic on a char boundary.
        let s = "ααααααααααβββββββββββγγγ";
        for w in 0..=s.chars().count() {
            let t = middle_truncate(s, w);
            assert!(t.chars().count() <= w.max(1), "width {w}: {t:?}");
        }
    }

    #[test]
    fn plurals_agree_with_their_count() {
        assert_eq!(plural(0, "item", "items"), "0 items");
        assert_eq!(plural(1, "item", "items"), "1 item");
        assert_eq!(plural(2, "item", "items"), "2 items");
        assert_eq!(plural(1000, "item", "items"), "1,000 items");
        assert_eq!(plural(50_000, "item", "items"), "50,000 items");
        assert_eq!(plural(1_234_567, "item", "items"), "1,234,567 items");
    }
}
