//! Reduced-motion detection (§2.11 rule 5).
//!
//! # What the spec requires
//!
//! "**Reduced motion:** every duration becomes 0 except the spinner, and every
//! animated property renders its final state immediately. No intermediate
//! keyframes are skipped-then-settled."
//!
//! # The honest state of this on Wayland
//!
//! There is **no portable query** for the desktop's animation preference, and
//! egui exposes none. What exists, in order of reliability:
//!
//! 1. **The XDG desktop portal `org.freedesktop.Appearance` setting** — the
//!    correct answer, reachable only over D-Bus, which means either a
//!    `zbus` dependency or a subprocess. A subprocess per frame is absurd; a
//!    `zbus` dependency for one boolean is a poor trade for a "lightweight"
//!    file manager.
//! 2. **`XDG_CURRENT_DESKTOP`** — a coarse hint about *which* DE, not about the
//!    user's preference. A GNOME session is not automatically motion-free.
//! 3. **`GTK_THEME`** — a *theme* name, not a motion preference. Reading
//!    `GTK_THEME=Adwaita:dark` to conclude "dark mode" is legitimate; reading
//!    it to conclude "no animations" is not.
//!
//! So: this module implements (2) and (3) as a **documented approximation**,
//! exposes the signal as a single overridable function, and says plainly in its
//! own docs that it is not the real thing. `KESTREL_REDUCED_MOTION` is the
//! escape hatch that makes the behaviour testable and scriptable today.
//!
//! # Why approximate is better than absent
//!
//! The animation budget in this app is small (a 900ms spinner, 130ms hovers),
//! and nothing in the shell animates *yet*. The point of wiring the signal now
//! is that Phase 4's animated code has a single place to ask, and that a user
//! who needs it has a documented switch on day one rather than a bug report on
//! day ninety.

/// The override: `1`/`true`/`yes` forces reduced motion on, `0` forces it off.
///
/// An explicit environment variable is the only *unambiguous* signal available
/// without a D-Bus dependency, and it is how a user with a specific need opts in
/// today.
pub const OVERRIDE_ENV: &str = "KESTREL_REDUCED_MOTION";

/// What we concluded, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// Animate: durations apply as specified.
    Full,
    /// Collapse every duration to zero; render final states immediately.
    Reduced,
}

impl Motion {
    /// `true` when durations must be treated as zero.
    #[must_use]
    pub fn is_reduced(self) -> bool {
        matches!(self, Motion::Reduced)
    }

    /// Applies the §2.11 rule 5 collapse to a duration.
    ///
    /// The spinner is the one documented exception: an indeterminate progress
    /// indicator that does not move is indistinguishable from a stalled
    /// process, which is a worse outcome than a moving dot for a user who asked
    /// for less motion but still needs to know work is happening.
    #[must_use]
    pub fn duration(self, d: std::time::Duration, spinner: bool) -> std::time::Duration {
        if self.is_reduced() && !spinner {
            std::time::Duration::ZERO
        } else {
            d
        }
    }
}

/// Reads the motion preference from the environment.
///
/// # Not the real thing — see the module docs.
///
/// This is the only function that touches the process environment;
/// [`classify`] holds the actual decision so the policy is testable without
/// mutating global state.
#[must_use]
pub fn detect() -> Motion {
    let override_value = std::env::var(OVERRIDE_ENV).ok();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let gtk = std::env::var("GTK_THEME").unwrap_or_default();
    classify(override_value.as_deref(), &desktop, &gtk)
}

/// The decision itself, with the environment passed in rather than read.
///
/// Split out so every branch is reachable from a test. `std::env::set_var` is
/// `unsafe` in edition 2024 and this crate sets `unsafe_code = "forbid"`, so
/// testing `detect` in place would mean either an `allow` or a lock across a
/// process-wide mutation racing every other test in the binary. Passing the
/// values in sidesteps both, and is the smaller change.
#[must_use]
pub fn classify(override_value: Option<&str>, desktop: &str, gtk_theme: &str) -> Motion {
    if let Some(raw) = override_value {
        return match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Motion::Reduced,
            "0" | "false" | "no" | "off" => Motion::Full,
            // An unparseable override is ignored rather than guessed at: the
            // variable exists to be authoritative, and silently treating a typo
            // as "reduced" would be a worse lie than treating it as unset.
            _ => Motion::Full,
        };
    }

    // Heuristic (2)/(3). **Deliberately a no-op today**, and that is the honest
    // answer rather than a placeholder: neither `XDG_CURRENT_DESKTOP` nor
    // `GTK_THEME` says anything about motion, so inferring from them would be a
    // guess dressed as a signal. A user who needs reduced motion sets
    // `KESTREL_REDUCED_MOTION`, and the status bar says "reduced motion" so the
    // state is visible rather than inferred.
    let _ = (desktop.to_ascii_lowercase(), gtk_theme.to_ascii_lowercase());
    Motion::Full
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn full_motion_leaves_durations_alone() {
        let m = Motion::Full;
        assert!(!m.is_reduced());
        assert_eq!(
            m.duration(Duration::from_millis(130), false),
            Duration::from_millis(130)
        );
        assert_eq!(
            m.duration(Duration::from_millis(900), true),
            Duration::from_millis(900)
        );
    }

    #[test]
    fn reduced_motion_zeroes_every_duration_except_the_spinner() {
        // §2.11 rule 5, verbatim.
        let m = Motion::Reduced;
        for d in tokens_motion_durations() {
            assert_eq!(m.duration(d, false), Duration::ZERO, "{d:?} must collapse");
        }
        assert_eq!(
            m.duration(Duration::from_millis(900), true),
            Duration::from_millis(900),
            "an indeterminate spinner must keep moving, or it reads as a stall"
        );
    }

    /// The transcribed §2.11 set, read from the token table rather than copied,
    /// so a new duration added to `tokens::motion` is covered here for free.
    fn tokens_motion_durations() -> Vec<Duration> {
        crate::tokens::motion::SUPPRESSED.to_vec()
    }

    #[test]
    fn the_override_is_authoritative_in_both_directions() {
        for (value, expect) in [
            ("1", Motion::Reduced),
            ("true", Motion::Reduced),
            ("YES", Motion::Reduced),
            ("yes", Motion::Reduced),
            ("on", Motion::Reduced),
            (" 1 ", Motion::Reduced),
            ("0", Motion::Full),
            ("false", Motion::Full),
            ("no", Motion::Full),
            ("off", Motion::Full),
        ] {
            assert_eq!(classify(Some(value), "", ""), expect, "override {value:?}");
        }
    }

    #[test]
    fn an_unparseable_override_falls_back_to_full_motion() {
        // A typo is not a preference. Treating it as "reduced" would be a worse
        // lie than treating it as unset.
        for value in ["maybe", "", "2", "reduced", "y"] {
            assert_eq!(classify(Some(value), "", ""), Motion::Full, "{value:?}");
        }
    }

    #[test]
    fn an_absent_override_never_reduces_motion() {
        // The documented state: no desktop signal is queryable, so the default
        // is Full and the *only* way to opt in is the override.
        assert_eq!(classify(None, "", ""), Motion::Full);
        assert_eq!(classify(None, "GNOME", "Adwaita:dark"), Motion::Full);
        assert_eq!(classify(None, "Hyprland", "adwaita-dark"), Motion::Full);
        assert_eq!(classify(None, "KDE", ""), Motion::Full);
    }

    #[test]
    fn the_override_beats_every_desktop_hint() {
        assert_eq!(
            classify(Some("1"), "GNOME", "Adwaita:dark"),
            Motion::Reduced,
            "an explicit request must not be second-guessed by a desktop hint"
        );
    }

    #[test]
    fn detection_never_panics_in_any_environment() {
        // Whatever the ambient environment is, this must not fail.
        let _ = detect();
    }
}
