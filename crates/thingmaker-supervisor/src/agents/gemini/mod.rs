//! Gemini, through Antigravity's official CLI, `agy`.
//!
//! In stream-json mode the session actor reaches `agy` through
//! [`bridge::GeminiBridge`]; native ACP uses the shared stdio transport. What is
//! Antigravity's own lives here: the launch contract (`launch`), the bridge,
//! and one-shot account questions (`probe`: sign-in and models).
//!
//! Sessions with session-local MCP servers use native `agy acp`; other
//! sessions retain the stream-json bridge. Team tokens travel only through
//! ACP session/new or session/load, never through shared configuration.
//!
//! The stream-json fixture was captured from `agy` 1.2.14. Native ACP
//! leadership is described in docs/plans/gemini-team-leadership.md.

pub mod bridge;
pub mod launch;
pub mod probe;
pub mod quota;

pub use launch::GeminiLaunchOptions;
