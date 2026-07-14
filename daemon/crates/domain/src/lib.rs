//! The domain: single source of truth for every type and pure rule in the
//! system. serde defines the wire format (IPC frames and DB payloads);
//! ts-rs exports the same types to TypeScript for the UI, so the two sides
//! can never drift.
//!
//! The wire format is decode-compatible with the records written by the
//! original F#/Fable codec (see tests/wire_compat.rs).

pub mod geometry;
pub mod grouping;
pub mod planning;
pub mod tagging;
pub mod types;
pub mod wire;

pub use types::*;
