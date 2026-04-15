//! ANSI-aware OSC 8 URL injector.
//!
//! Scans a byte stream for URLs and wraps them in OSC 8 hyperlink escape
//! sequences so terminals like WezTerm and iTerm2 can follow visually-wrapped
//! URLs via ctrl+click. The scanner is aware of ANSI CSI / OSC escape
//! sequences and never injects inside them, and it skips URLs that are already
//! inside an existing OSC 8 span to avoid double-wrapping.
//!
//! The [`Injector`] is a pure stream transformer — it holds no I/O. Feed it
//! bytes with [`Injector::push`] and call [`Injector::flush`] at end of stream
//! (or whenever you want any held trailing bytes emitted verbatim).

mod inject;

pub use inject::{Injector, InjectorState};
