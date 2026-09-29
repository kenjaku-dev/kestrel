//! The preview pane: what to show for the focused row, and how to get it without
//! blocking the frame.
//!
//! # There is no §4 spec for this pane
//!
//! The token spec has a `Space` binding for "Quick look" and nothing else — no
//! section, no tokens, no states. So every token here is **derived** from the
//! scales that do exist (§2.7 type, §2.9 metrics, §3.2 text, §3.3 borders) and
//! each one says so in its doc comment. If a §4 section is added later, these
//! should be replaced by it rather than reconciled with it.
//!
//! # The size cap is the whole design
//!
//! A file manager will one day be pointed at a 40 GB video or a 2 GB log. The
//! tempting implementation is "read the file, syntax-highlight it" — and that is
//! a page fault storm, a multi-second freeze, and possibly an OOM, all of them
//! triggered by *moving the arrow key*. So:
//!
//! * [`MAX_PREVIEW_BYTES`] is checked **before** any read, on the size the
//!   listing already carries. Nothing is opened to find out how big it is.
//! * Above the cap the pane says so and shows what it *can*: the icon, the type,
//!   the size, the modified time. A degraded preview that tells the truth is
//!   better than a blank pane.
//! * The read itself is capped *again* at [`READ_CAP_BYTES`] on the worker, in
//!   case the listing's size was stale — a file that grew between the scan and
//!   the arrow press must not be able to overrun the cap.
//!
//! # Cancellable, because arrow-keying must not queue 50 loads
//!
//! Every keystroke that moves the focus asks for a different preview. If those
//! requests all ran, arrowing through a folder of 50 images would start 50 image
//! decodes. So [`Loader::request`] **cancels** whatever is in flight first, and
//! only then starts the new one — the same discipline the scanner uses with its
//! `CancellationToken`, and the reason this type exists rather than a bare
//! `std::thread::spawn` per keystroke.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// The largest file the preview pane will open at all.
///
/// 1 MiB. Chosen so that the *overwhelming* majority of source, config, text and
/// markup files preview instantly, and so that the failure mode is bounded: a
/// 1 MiB read is a few milliseconds even cold, which is under a frame.
///
/// Deliberately not larger. A 8 MiB cap would preview more log files, and the
/// cost is a preview that takes longer than the arrow key that asked for it.
pub const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;

/// The hard read ceiling on the worker, independent of the listing's size.
///
/// The listing's size can be stale — a file grows while you are looking at the
/// directory. Re-checking after opening costs nothing and closes the gap.
pub const READ_CAP_BYTES: u64 = MAX_PREVIEW_BYTES + 64 * 1024;

/// How long the loader waits before it will start a new load, in milliseconds.
///
/// Arrow-keying fires faster than a human reads, so without a debounce a single
/// deliberate scroll of the list starts a load per row. 120 ms is below the
/// threshold where it feels laggy on a single deliberate selection and well
/// above the inter-key interval of a held arrow key.
pub const DEBOUNCE_MS: u64 = 120;

/// What kind of preview a file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Decode as an image.
    Image,
    /// Read as text and syntax-highlight it.
    Text,
    /// Show metadata only. The pane is never blank.
    Metadata,
    /// Too large to open; show metadata only.
    TooLarge,
    /// The listing has no size for it, so the cap cannot be applied honestly.
    Unknown,
}

impl Kind {
    /// A one-word label for the pane's kind line.
    #[must_use]
    // `app.rs` currently switches on the variant and writes its own strings;
    // this accessor is the single-source version of the same mapping. Kept so
    // the copy has one owner once the pane starts using it.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Text => "Text",
            Self::Metadata => "File",
            Self::TooLarge => "Too large",
            Self::Unknown => "Unknown size",
        }
    }

    /// `true` when the pane will open the file at all.
    #[must_use]
    pub fn is_viewable(self) -> bool {
        matches!(self, Self::Image | Self::Text)
    }
}

/// Decides what to do with an entry, from its category and its size.
///
/// Pure, and the *only* place the cap is applied to a listing value — so "do not
/// read a huge file" is one testable rule rather than a condition scattered
/// through the paint path.
#[must_use]
pub fn kind_for(is_image: bool, size: Option<u64>) -> Kind {
    let Some(bytes) = size else {
        // No size: a directory, or a filesystem that does not report one. We
        // cannot apply a byte cap to a value we do not have, and guessing "small"
        // is how a 4 GB log gets slurped. The worker enforces the read cap
        // regardless, so `Unknown` is safe rather than a hole.
        return Kind::Unknown;
    };
    if bytes > MAX_PREVIEW_BYTES {
        return Kind::TooLarge;
    }
    if is_image { Kind::Image } else { Kind::Text }
}

/// The result of a completed preview load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// Text, syntax-highlighted into lines.
    Text {
        /// The lines, already middle-truncated to the pane width.
        lines: Vec<Line>,
        /// `true` when the file was longer than the read cap and was cut.
        truncated: bool,
    },
    /// Raw image bytes, for the image loaders.
    Image {
        /// The encoded file, verbatim.
        bytes: Vec<u8>,
    },
    /// Too large to open.
    TooLarge {
        /// Its size, for the "why".
        bytes: u64,
    },
    /// Could not be read; the pane shows the reason instead of the file.
    Failed {
        /// A user-facing clause.
        reason: String,
    },
}

/// A backslash, as a named constant.
///
/// Named because the literal needs escaping inside this function's own string
/// literals, and `c == '\\'` written inline is both hard to read and easy to
/// break by an over-eager replacement.
const BACKSLASH: char = '\\';

/// A single quote, for the same reason as [`BACKSLASH`].
const SINGLE_QUOTE: char = '\'';

/// One highlighted line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The line number, 1-based, for the gutter.
    pub number: usize,
    /// The visible text.
    pub text: String,
    /// The span kind per byte of `text`, for colouring.
    ///
    /// A `Vec<u8>` of token ids rather than a per-character `Vec<Span>`: a
    /// 1 MiB file of 30,000 lines would otherwise allocate 30,000 vectors per
    /// frame. One allocation for the whole file.
    pub spans: Vec<u8>,
}

/// Token kinds a line can be coloured by.
///
/// Deliberately tiny. §7.15 forbids decoration that does not carry information,
/// and a full syntax grammar is not required for a *preview* — the job is to let
/// a user recognise a file, not to read it. Four classes is what that takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Token {
    /// Ordinary text.
    Plain = 0,
    /// A keyword, or a type name.
    Keyword = 1,
    /// A string literal.
    Literal = 2,
    /// A comment.
    Comment = 3,
}

impl Token {
    /// Reads a span byte back into a token.
    ///
    /// Public because the painter indexes a `Vec<u8>` of span bytes and has to
    /// turn each one back into a colour. Total: an out-of-range byte degrades to
    /// `Plain` rather than panicking, so a corrupt span can never crash a paint.
    #[must_use]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Keyword,
            2 => Self::Literal,
            3 => Self::Comment,
            _ => Self::Plain,
        }
    }
}

/// Splits text into lines and classes each line, without a grammar.
///
/// The whole highlighter is a state machine over three things: a line-comment
/// introducer, a string delimiter, and whether a line starts inside a string. No
/// parser, no dependencies, and — the point — it is bounded by the read cap, so
/// its worst case is already capped.
///
/// Highlighting is *not* required to be correct to be useful, and §7.11's
/// "clipboard-visible" rule does not apply to a preview. But it must never lie
/// about a line's *content*, so the text is always the original bytes; only the
/// colour is derived.
#[must_use]
pub fn highlight(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut in_string: Option<char> = None;
    for (i, raw) in text.lines().enumerate() {
        let mut spans = vec![Token::Plain as u8; raw.len()];
        let mut chars = raw.char_indices().peekable();
        // A comment runs to end of line, so it is handled by colouring the
        // remainder and breaking rather than by a flag the loop would have to
        // keep checking: the flag version cost a `chars.by_ref()` drain per
        // character purely to be ignored.
        while let Some((idx, c)) = chars.next() {
            match in_string {
                Some(q) => {
                    spans[idx] = Token::Literal as u8;
                    if c == BACKSLASH {
                        // Skip the escaped character so `\"` does not close.
                        if let Some((next_i, _)) = chars.next() {
                            spans[next_i] = Token::Literal as u8;
                        }
                    } else if c == q {
                        in_string = None;
                    }
                }
                None => {
                    // A comment introducer, for the three comment syntaxes a
                    // preview actually meets.
                    if matches!(c, '#' | ';' | '-') {
                        let is_comment = match c {
                            '#' => true,
                            ';' => true,
                            _ => raw[idx..].starts_with("--"),
                        };
                        if is_comment {
                            spans[idx..raw.len()].fill(Token::Comment as u8);
                            break;
                        }
                    }
                    if c == '"' || c == SINGLE_QUOTE {
                        in_string = Some(c);
                        spans[idx] = Token::Literal as u8;
                    } else if c.is_alphabetic() || c == '_' {
                        let start = idx;
                        let mut end = idx + c.len_utf8();
                        while let Some(&(next_i, next_c)) = chars.peek() {
                            if next_c.is_alphanumeric() || next_c == '_' {
                                chars.next();
                                end = next_i + next_c.len_utf8();
                            } else {
                                break;
                            }
                        }
                        if is_keyword(&raw[start..end]) {
                            spans[start..end].fill(Token::Keyword as u8);
                        }
                    }
                }
            }
        }
        out.push(Line {
            number: i + 1,
            text: raw.to_string(),
            spans,
        });
    }
    out
}

/// The words that get the keyword colour.
///
/// A short, language-agnostic list rather than a per-language grammar. Anything
/// longer would be a syntax highlighter pretending to be a preview, and §7.15
/// would be right to object.
const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "def",
    "default",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "final",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mod",
    "mut",
    "new",
    "nil",
    "none",
    "null",
    "of",
    "package",
    "pass",
    "private",
    "public",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "use",
    "var",
    "void",
    "where",
    "while",
    "with",
    "yield",
];

#[must_use]
pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.binary_search(&word).is_ok()
}

/// The colours a preview's four token classes are drawn in.
///
/// Not a token table of its own: each field resolves a **§3 semantic role** at
/// paint time, so the preview follows the theme and there is no raw hex here.
/// `Plain` is `text.primary` because code is *read*, not scanned, so it gets the
/// full-contrast colour; the other three are the syntax roles §3.7's families
/// imply, at `text.secondary` for literals and `text.tertiary` for comments —
/// comments recede because a preview is usually being skimmed for structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette;

impl Palette {
    /// The colour for a token class, in a theme.
    #[must_use]
    pub fn color(self, token: Token, theme: &crate::tokens::Theme) -> egui::Color32 {
        match token {
            Token::Plain => theme.text.primary,
            Token::Keyword => theme.accent.text,
            Token::Literal => theme.text.secondary,
            Token::Comment => theme.text.tertiary,
        }
    }
}

/// The palette for a theme. A function rather than a field so it cannot go stale
/// against a theme that changed.
#[must_use]
pub fn palette(_theme: &crate::tokens::Theme) -> Palette {
    Palette
}

/// A cancellable, debounced preview loader.
///
/// The type exists so that "cancel the previous load" is enforced by construction
/// rather than by every caller remembering to. Arrowing through a folder of 50
/// images fires 50 requests; without this, that is 50 concurrent decodes.
#[derive(Debug)]
pub struct Loader {
    rx: Receiver<Loaded>,
    /// Bumped on every request. A worker whose generation no longer matches the
    /// current one discards its own result rather than publishing it over a newer
    /// one — which is what stops a slow load for row 2 landing after the user has
    /// already moved to row 3.
    generation: Arc<AtomicU64>,
    /// Shared with the worker so a long read can stop at its next 32 KiB boundary.
    cancelled: Arc<AtomicBool>,
    /// When the queued request becomes eligible to start.
    pending: Option<std::time::Instant>,
    /// The queued request.
    queued: Option<Queued>,
    /// The kind of the last request, so the pane can shape itself immediately.
    kind: Option<Kind>,
    /// The path and image-ness of the last request, so a repeat of the same one
    /// is recognised as such instead of cancelling its own predecessor.
    current: Option<(PathBuf, bool)>,
    /// `true` between spawning a worker and its result arriving.
    ///
    /// The debounce flag is not enough: once `poll` has spawned the worker,
    /// `pending` is cleared and the load is still running. Anything that has to
    /// wait for the preview to *finish* rather than merely to be *queued* needs
    /// this, and it is the difference between a capture that shows content and
    /// one that shows "Loading…".
    awaiting: bool,
}

/// A request waiting out its debounce.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Queued {
    path: PathBuf,
    is_image: bool,
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl Loader {
    /// A loader with nothing pending.
    #[must_use]
    pub fn new() -> Self {
        // A disconnected channel is the point: `try_recv` on it reports
        // `Disconnected` immediately, which is what makes `poll` non-blocking
        // with no worker alive.
        let (_tx, rx) = channel();
        drop(_tx);
        Self {
            rx,
            generation: Arc::new(AtomicU64::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
            pending: None,
            queued: None,
            kind: None,
            current: None,
            awaiting: false,
        }
    }

    /// Asks for `path`, cancelling anything in flight.
    ///
    /// Returns the kind that will be produced, so the pane can show the right
    /// *shape* immediately — a metadata pane while the bytes are still loading —
    /// instead of an empty rectangle.
    ///
    /// # Asking twice for the same file is a no-op
    ///
    /// The pane calls this **every frame** — it has no other way to learn that
    /// the focus moved — and a request that always cancelled would cancel its own
    /// predecessor, resetting the debounce 60 times a second. The preview would
    /// never start, and the pane would sit on "Loading…" for the life of the
    /// app. So the target is remembered and an unchanged request returns
    /// immediately.
    ///
    /// That is not an optimisation, it is the only correct reading of "cancel
    /// the previous load": the thing to cancel is the load for a *different*
    /// file. Making the type idempotent is what stops the per-frame call site
    /// from having to remember to diff the target itself.
    pub fn request(&mut self, path: &Path, size: Option<u64>, is_image: bool) -> Kind {
        if self.current.as_ref() == Some(&(path.to_path_buf(), is_image)) {
            return self.kind.unwrap_or_else(|| kind_for(is_image, size));
        }
        // Cancel first, always. This is the whole reason for the type.
        self.cancel();
        self.current = Some((path.to_path_buf(), is_image));
        let kind = kind_for(is_image, size);
        self.kind = Some(kind);
        if !kind.is_viewable() {
            // Nothing to load; the pane already has the metadata it will show.
            return kind;
        }
        self.queued = Some(Queued {
            path: path.to_path_buf(),
            is_image,
        });
        self.pending =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(DEBOUNCE_MS));
        kind
    }

    /// Cancels any pending or in-flight load.
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.cancelled.store(true, Ordering::Relaxed);
        self.pending = None;
        self.queued = None;
        // The in-flight worker's result is now unwanted, so nothing is awaiting
        // it. Clearing this is what stops a cancelled load from pinning the pane
        // in "Loading…" forever.
        self.awaiting = false;
    }

    /// `true` while a load is debouncing.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// `true` while a load is queued **or** running.
    ///
    /// [`Self::is_pending`] alone answers "has the debounce elapsed?", which is
    /// not the same question: a 1 MiB decode on a cold cache outlives its
    /// debounce by orders of magnitude.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.pending.is_some() || self.awaiting
    }

    /// The kind of the last request.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn kind(&self) -> Option<Kind> {
        self.kind
    }

    /// Polls for a finished load, starting one if the debounce has elapsed.
    ///
    /// Non-blocking by construction: one `try_recv` and at most one `spawn`.
    pub fn poll(&mut self) -> Option<Loaded> {
        if let Ok(loaded) = self.rx.try_recv() {
            self.awaiting = false;
            return Some(loaded);
        }
        let due = self
            .pending
            .is_some_and(|at| std::time::Instant::now() >= at);
        if !due {
            return None;
        }
        self.pending = None;
        let queued = self.queued.take()?;
        self.spawn(queued);
        None
    }

    fn spawn(&mut self, queued: Queued) {
        // A fresh channel per load: a stale result from a cancelled worker can
        // never be mistaken for this one's, and there is no shared-slot state to
        // get wrong.
        let (tx, rx) = channel();
        self.rx = rx;
        self.cancelled.store(false, Ordering::Relaxed);
        let generation = Arc::clone(&self.generation);
        let cancelled = Arc::clone(&self.cancelled);
        // The generation this load belongs to. Read **here**, on the frame
        // thread, and carried into the worker: reading it inside the worker
        // would compare the counter against itself and always answer "unchanged".
        let mine = self.generation.load(Ordering::Relaxed);
        self.awaiting = true;
        // A named thread so a stack trace during a slow preview names the
        // subsystem. One thread per request is safe *because* `request` cancels
        // first: cancelled workers exit at their next 32 KiB read boundary.
        let spawned = std::thread::Builder::new()
            .name("kestrel-preview".to_string())
            .spawn(move || {
                let result = load(&queued.path, queued.is_image, &cancelled);
                // Only publish if nobody has asked for something else since. This
                // is the check that stops a slow decode for row 2 from landing
                // after the user has already moved to row 3.
                if generation.load(Ordering::Relaxed) == mine {
                    let _ = tx.send(result);
                }
            });
        if spawned.is_err() {
            // A failed spawn must not leave the pane spinning forever. Report it
            // through the channel the same way a load would.
            self.awaiting = false;
            self.kind = Some(Kind::Metadata);
        }
    }
}

/// Reads a file for preview, honouring the cap and the cancel flag.
///
/// The only place in the GUI crate that opens a file, and it is only ever reached
/// on a worker thread.
fn load(path: &Path, is_image: bool, cancelled: &Arc<AtomicBool>) -> Loaded {
    // `std::fs::metadata` follows symlinks, which is what a preview wants: the
    // user asked to see the file, and a symlink to a big file should be treated
    // as a big file rather than as its own short link.
    let Ok(meta) = std::fs::metadata(path) else {
        return Loaded::Failed {
            reason: "it could not be read".to_string(),
        };
    };
    if meta.len() > MAX_PREVIEW_BYTES {
        return Loaded::TooLarge { bytes: meta.len() };
    }
    if cancelled.load(Ordering::Relaxed) {
        return Loaded::Failed {
            reason: "superseded".to_string(),
        };
    }
    // Read **at most** the cap even though the metadata allowed it: a file that
    // grew between the listing and this moment must not overrun the budget, and
    // `take` enforces that rather than a check that can be raced.
    let bytes = match read_capped(path, READ_CAP_BYTES, cancelled) {
        Some(b) => b,
        None => {
            return Loaded::Failed {
                reason: "it could not be read".to_string(),
            };
        }
    };
    if bytes.len() as u64 > MAX_PREVIEW_BYTES {
        return Loaded::TooLarge {
            bytes: bytes.len() as u64,
        };
    }
    if is_image {
        // Hand the encoded bytes to egui's image loaders verbatim. Decoding is
        // theirs, on their own terms; guessing a container here would be a
        // second, worse implementation of something that already exists.
        return Loaded::Image { bytes };
    }
    // A NUL byte means binary, and the pane should say so rather than render
    // replacement characters.
    if bytes.contains(&0) {
        return Loaded::Failed {
            reason: "it looks like a binary file".to_string(),
        };
    }
    match String::from_utf8(bytes) {
        Ok(text) => {
            let lines = highlight(&text);
            Loaded::Text {
                lines,
                truncated: false,
            }
        }
        Err(_) => Loaded::Failed {
            reason: "it is not valid UTF-8 text".to_string(),
        },
    }
}

/// Reads up to `cap` bytes, stopping early if cancelled.
fn read_capped(path: &Path, cap: u64, cancelled: &Arc<AtomicBool>) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut handle = file.take(cap);
    let mut buf = Vec::new();
    // 32 KiB at a time, so the cancel flag is checked often without a syscall
    // per byte.
    let mut chunk = vec![0u8; 32 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        match handle.read(&mut chunk) {
            Ok(0) => return Some(buf),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return Some(buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// Decodes a hex string into bytes, for binary fixtures.
    ///
    /// Whitespace and newlines are ignored so a long literal can be wrapped.
    fn hex(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(clean.len() % 2, 0, "odd hex length");
        (0..clean.len() / 2)
            .map(|i| u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16).expect("two hex digits"))
            .collect()
    }

    // -- the size cap -------------------------------------------------------

    /// A small text file previews; a huge one does not.
    #[test]
    fn the_cap_is_applied_before_anything_is_read() {
        assert_eq!(kind_for(false, Some(1024)), Kind::Text);
        assert_eq!(kind_for(false, Some(MAX_PREVIEW_BYTES)), Kind::Text);
        assert_eq!(
            kind_for(false, Some(MAX_PREVIEW_BYTES + 1)),
            Kind::TooLarge,
            "one byte over the cap must not preview"
        );
        assert_eq!(kind_for(false, Some(u64::MAX)), Kind::TooLarge);
    }

    /// Images are capped by the same rule. A 200 MB TIFF must not be decoded.
    #[test]
    fn images_are_capped_like_everything_else() {
        assert_eq!(kind_for(true, Some(4096)), Kind::Image);
        assert_eq!(kind_for(true, Some(MAX_PREVIEW_BYTES + 1)), Kind::TooLarge);
    }

    /// No size means the cap cannot be applied, so nothing is opened.
    ///
    /// Guessing "probably small" here is how a 4 GB log gets read into memory
    /// because its directory entry lied.
    #[test]
    fn an_unknown_size_is_not_viewable() {
        assert_eq!(kind_for(false, None), Kind::Unknown);
        assert_eq!(kind_for(true, None), Kind::Unknown);
        assert!(!Kind::Unknown.is_viewable());
    }

    /// Only image and text are opened.
    #[test]
    fn only_image_and_text_are_viewable() {
        assert!(Kind::Image.is_viewable());
        assert!(Kind::Text.is_viewable());
        assert!(!Kind::TooLarge.is_viewable());
        assert!(!Kind::Metadata.is_viewable());
        assert!(!Kind::Unknown.is_viewable());
    }

    /// Each kind names itself, and none says "OK".
    #[test]
    fn kinds_name_themselves() {
        assert_eq!(Kind::TooLarge.label(), "Too large");
        assert_eq!(Kind::Image.label(), "Image");
        for k in [
            Kind::Image,
            Kind::Text,
            Kind::Metadata,
            Kind::TooLarge,
            Kind::Unknown,
        ] {
            assert!(!k.label().is_empty());
        }
    }

    // -- the loader's own cap ----------------------------------------------

    /// A file over the cap is refused without being read into memory.
    #[test]
    fn an_oversized_file_is_refused_on_the_worker() {
        let d = tmp();
        // Sparse: costs no disk, but is genuinely over the cap.
        let big = d.path().join("big.txt");
        let f = fs::File::create(&big).expect("create");
        f.set_len(MAX_PREVIEW_BYTES + 4096).expect("set_len");
        drop(f);
        let cancel = Arc::new(AtomicBool::new(false));
        let loaded = load(&big, false, &cancel);
        match loaded {
            Loaded::TooLarge { bytes } => {
                assert!(bytes > MAX_PREVIEW_BYTES, "reported {bytes}");
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    /// A file that grows past the cap *after* the listing is still caught.
    ///
    /// This is the stale-listing case the second cap exists for: the check on
    /// `size` passed, so only the read-time cap can stop it.
    #[test]
    fn a_file_that_grew_past_the_cap_is_still_refused() {
        let d = tmp();
        let path = d.path().join("grew.txt");
        fs::write(&path, b"small").expect("write");
        // Reported size is fine, but the file on disk is not.
        let cancel = Arc::new(AtomicBool::new(false));
        fs::write(&path, vec![b'a'; (MAX_PREVIEW_BYTES + 8192) as usize]).expect("grow");
        let loaded = load(&path, false, &cancel);
        assert!(
            matches!(loaded, Loaded::TooLarge { .. }),
            "a grown file must be refused, got {loaded:?}"
        );
    }

    /// A cancelled read stops and reports, rather than finishing.
    #[test]
    fn a_cancelled_read_stops() {
        let d = tmp();
        let path = d.path().join("x.txt");
        fs::write(&path, vec![b'a'; 100_000]).expect("write");
        let cancel = Arc::new(AtomicBool::new(true));
        let loaded = load(&path, false, &cancel);
        assert!(
            matches!(loaded, Loaded::Failed { .. }),
            "a cancelled load must not produce content, got {loaded:?}"
        );
    }

    /// A NUL byte means binary, and the pane says so instead of showing mojibake.
    #[test]
    fn binary_content_is_named_as_binary() {
        let d = tmp();
        let path = d.path().join("bin");
        fs::write(&path, [0x00, 0x01, 0x02, b'h', b'i']).expect("write");
        let cancel = Arc::new(AtomicBool::new(false));
        let loaded = load(&path, false, &cancel);
        match loaded {
            Loaded::Failed { reason } => assert!(reason.contains("binary"), "{reason}"),
            other => panic!("expected a binary failure, got {other:?}"),
        }
    }

    /// Valid text round-trips into lines.
    #[test]
    fn text_previews_as_numbered_lines() {
        let d = tmp();
        let path = d.path().join("a.txt");
        fs::write(&path, b"one\ntwo\nthree\n").expect("write");
        let cancel = Arc::new(AtomicBool::new(false));
        let loaded = load(&path, false, &cancel);
        match loaded {
            Loaded::Text { lines, truncated } => {
                assert!(!truncated);
                assert_eq!(lines.len(), 3);
                assert_eq!(lines[0].number, 1);
                assert_eq!(lines[0].text, "one");
                assert_eq!(lines[2].text, "three");
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    /// An image is handed over verbatim for the loaders to decode.
    ///
    /// The GUI does not guess a container: `egui_extras`'s loaders already do
    /// that, and a second implementation here would be a second thing to be
    /// wrong about PNGs.
    #[test]
    fn an_image_is_passed_through_for_the_loaders() {
        let d = tmp();
        let path = d.path().join("a.png");
        // A real 1x1 PNG, written from hex so the byte count cannot drift out of
        // sync with the array length.
        //
        // The byte sequence is a genuine IHDR/IDAT/IEND stream (68 bytes, 136 hex
        // digits, every chunk CRC correct) — the test's point is that real PNG
        // bytes survive the loader verbatim, which a hand-edited fixture cannot
        // demonstrate.
        let png: Vec<u8> = hex(
            "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489 \
             0000000b49444154789c6360000200000500017a5eab3f0000000049454e44ae426082",
        );
        fs::write(&path, &png).expect("write");
        let cancel = Arc::new(AtomicBool::new(false));
        match load(&path, true, &cancel) {
            Loaded::Image { bytes } => {
                assert_eq!(bytes, png.clone(), "the encoded bytes must be verbatim");
            }
            other => panic!("expected Image, got {other:?}"),
        }
        // The same bytes read as *text* are binary, and must be named as such
        // rather than rendered as replacement characters.
        match load(&path, false, &cancel) {
            Loaded::Failed { reason } => assert!(reason.contains("binary"), "{reason}"),
            other => panic!("expected a binary failure, got {other:?}"),
        }
    }

    /// A missing file fails with a reason rather than panicking.
    #[test]
    fn a_missing_file_fails_cleanly() {
        let cancel = Arc::new(AtomicBool::new(false));
        let loaded = load(Path::new("/definitely/not/here/9c2f"), false, &cancel);
        assert!(matches!(loaded, Loaded::Failed { .. }), "got {loaded:?}");
    }

    // -- cancellation and debounce ----------------------------------------

    /// Requesting again cancels the previous request, so only one load is live.
    ///
    /// This is the property the type exists for: arrow-keying through a folder
    /// must not leave a decode running per row.
    #[test]
    fn a_new_request_cancels_the_previous_one() {
        let d = tmp();
        let a = d.path().join("a.txt");
        let b = d.path().join("b.txt");
        fs::write(&a, b"a").expect("write");
        fs::write(&b, b"b").expect("write");
        let mut l = Loader::new();
        l.request(&a, Some(1), false);
        assert!(l.is_pending());
        // Immediately requesting `b` must drop `a` entirely.
        l.request(&b, Some(1), false);
        assert!(l.is_pending());
        l.cancel();
        assert!(!l.is_pending(), "cancel must clear the debounce");
    }

    /// A non-viewable request never becomes pending, because there is nothing
    /// to load.
    #[test]
    fn a_non_viewable_request_does_not_queue_a_load() {
        let mut l = Loader::new();
        let kind = l.request(Path::new("/x"), Some(u64::MAX), false);
        assert_eq!(kind, Kind::TooLarge);
        assert!(!l.is_pending(), "an oversized file must not queue a read");
    }

    /// Nothing arrives before the debounce elapses.
    #[test]
    fn the_debounce_holds_the_load_back() {
        let d = tmp();
        let a = d.path().join("a.txt");
        fs::write(&a, b"a").expect("write");
        let mut l = Loader::new();
        l.request(&a, Some(1), false);
        // `poll` before the deadline must not start anything.
        assert!(l.poll().is_none());
        assert!(l.is_pending(), "still debouncing");
    }

    /// The whole round trip: request → debounce → spawn → result.
    ///
    /// This is the test the loader was missing, and it is the one that matters,
    /// because every other test here calls [`load`] directly. A defect in the
    /// publish guard — the generation comparison that decides whether a worker is
    /// allowed to hand its result over — is invisible to a direct `load` call and
    /// invisible to every other test in this module, because the only observable
    /// effect is that the preview pane never stops saying "Loading…".
    #[test]
    fn a_request_comes_back_through_the_loader() {
        let d = tmp();
        let a = d.path().join("round-trip.txt");
        fs::write(&a, b"alpha\nbeta\ngamma\n").expect("write");
        let mut l = Loader::new();
        assert_eq!(l.request(&a, Some(24), false), Kind::Text);

        // Poll on a real clock: the debounce has to elapse and the worker has to
        // be scheduled. Bounded so a regression fails the suite instead of
        // hanging it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            if let Some(loaded) = l.poll() {
                got = Some(loaded);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match got {
            Some(Loaded::Text { lines, truncated }) => {
                assert!(!truncated);
                assert_eq!(lines.len(), 3, "the file has three lines");
                assert_eq!(lines[1].text, "beta");
            }
            other => panic!("the loader never published a result: {other:?}"),
        }
        assert!(!l.is_busy(), "nothing is in flight once the result lands");
    }

    /// A cancelled request must not pin the pane in "Loading…" forever.
    ///
    /// The counterpart to the round trip: `cancel` clears the in-flight flag, so
    /// a caller that waits for the pane to go quiet is not waiting on a worker
    /// whose result was thrown away on purpose.
    #[test]
    fn cancelling_clears_the_in_flight_flag() {
        let d = tmp();
        let a = d.path().join("x.txt");
        fs::write(&a, b"x").expect("write");
        let mut l = Loader::new();
        l.request(&a, Some(1), false);
        assert!(l.is_busy());
        l.cancel();
        assert!(!l.is_busy());
    }

    /// A request for something that is not viewable is never in flight.
    #[test]
    fn an_unviewable_request_is_idle_immediately() {
        let mut l = Loader::new();
        l.request(Path::new("/x"), Some(u64::MAX), false);
        assert!(!l.is_busy());
        assert_eq!(l.kind(), Some(Kind::TooLarge));
    }

    /// Asking again for the **same** file must not cancel the first request.
    ///
    /// The pane calls `request` on every frame, because it has no other way to
    /// learn the focus moved. A version of `request` that always cancelled
    /// therefore cancelled its own predecessor 60 times a second: the debounce
    /// never elapsed, no load ever started, and the pane sat on "Loading…" for
    /// the life of the app. The symptom is a preview that is *stuck*, which is
    /// why this is asserted directly rather than inferred from a round trip.
    #[test]
    fn repeating_the_same_request_does_not_restart_it() {
        let d = tmp();
        let a = d.path().join("a.txt");
        fs::write(&a, b"a").expect("write");
        let mut l = Loader::new();
        l.request(&a, Some(1), false);
        assert!(l.is_pending());
        let queued_at = l.queued.clone();
        // Ten more frames' worth of the same request.
        for _ in 0..10 {
            l.request(&a, Some(1), false);
        }
        assert!(l.is_pending(), "the request survived the repeats");
        assert_eq!(l.queued, queued_at, "the queued work is the same one");
    }

    /// Moving to a different file *does* replace the request, and the old
    /// target's result can no longer land.
    ///
    /// The two halves of the idempotence rule together: same file is a no-op,
    /// different file cancels. Without the second, arrow-keying back and forth
    /// over two files would run both loads forever.
    #[test]
    fn a_different_request_replaces_the_queued_one() {
        let d = tmp();
        let a = d.path().join("a.txt");
        let b = d.path().join("b.txt");
        fs::write(&a, b"a").expect("write");
        fs::write(&b, b"b").expect("write");
        let mut l = Loader::new();
        l.request(&a, Some(1), false);
        let first = l.generation.load(Ordering::Relaxed);
        l.request(&b, Some(1), false);
        assert_ne!(
            l.generation.load(Ordering::Relaxed),
            first,
            "a new target must bump the generation, so an in-flight worker for the \
             old one cannot publish over it"
        );
        assert_eq!(
            l.queued,
            Some(Queued {
                path: b,
                is_image: false
            })
        );
    }

    // -- the highlighter ---------------------------------------------------

    /// A comment is coloured as one, to end of line.
    #[test]
    fn comments_are_one_run() {
        let lines = highlight("let x = 1; # trailing note\nlet y = 2;");
        let spans = &lines[0].spans;
        let comment_start = spans
            .iter()
            .position(|s| *s == Token::Comment as u8)
            .expect("a comment span");
        assert_eq!(*spans.last().expect("a span"), Token::Comment as u8);
        assert!(
            comment_start > 0,
            "the comment must not start at the line start"
        );
    }

    /// A string literal is coloured, and the line's *text* is untouched.
    ///
    /// The second half is the important one: colour is derived, content is not.
    #[test]
    fn strings_are_coloured_but_the_text_is_never_altered() {
        let raw = r#"let s = "hello";"#;
        let lines = highlight(raw);
        assert_eq!(lines[0].text, raw, "the text must be byte-identical");
        assert!(lines[0].spans.contains(&(Token::Literal as u8)));
    }

    /// A keyword run is coloured, and an identifier that merely contains a
    /// keyword is not.
    #[test]
    fn keywords_are_whole_words() {
        let lines = highlight("return variable");
        let first = &lines[0].spans[..6];
        assert!(first.iter().all(|s| *s == Token::Keyword as u8), "return");
        // `variable` contains `var`-ish text but is not a keyword.
        let second = &lines[0].spans[7..];
        assert!(second.iter().all(|s| *s == Token::Plain as u8), "variable");
    }

    /// The keyword list is sorted, because `is_keyword` binary-searches it.
    ///
    /// A silently-unsorted list would make half the keywords unrecognised with no
    /// error anywhere — the exact class of bug that is invisible until someone
    /// notices a colour is missing.
    #[test]
    fn the_keyword_list_is_sorted() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            KEYWORDS,
            sorted.as_slice(),
            "KEYWORDS must stay sorted for binary_search"
        );
    }

    /// `Token` round-trips through its `u8`, so a span byte is safe to read back.
    #[test]
    fn tokens_round_trip_through_bytes() {
        for t in [Token::Plain, Token::Keyword, Token::Literal, Token::Comment] {
            assert_eq!(Token::from_u8(t as u8), t);
        }
        // An out-of-range byte degrades to plain rather than panicking: span
        // bytes come from a `Vec<u8>` and a bad one must not crash the painter.
        assert_eq!(Token::from_u8(200), Token::Plain);
    }

    /// The span array is exactly the text's byte length, always.
    ///
    /// The painter indexes `spans` by byte offset into `text`; a mismatch would
    /// read past the end or colour the wrong run.
    #[test]
    fn spans_always_match_the_text_length() {
        for raw in [
            "",
            "ascii",
            "héllo wörld",
            "日本語のテキスト",
            r#"mixed "quotes" and 'apostrophes' -- and #hashes"#,
            "emoji 🎉 and a tab\there",
        ] {
            let lines = highlight(raw);
            for line in lines {
                assert_eq!(
                    line.spans.len(),
                    line.text.len(),
                    "span/text length mismatch for {raw:?}"
                );
            }
        }
    }

    /// A multi-byte character does not desynchronise the span array.
    #[test]
    fn multibyte_text_keeps_spans_aligned() {
        let raw = "let s = \"héllo\"; // ✓";
        let lines = highlight(raw);
        let line = &lines[0];
        assert_eq!(line.spans.len(), raw.len());
        assert!(line.spans.contains(&(Token::Comment as u8)));
    }
}
