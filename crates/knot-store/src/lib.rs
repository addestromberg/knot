//! Filesystem adapter. Opening a backend holds an exclusive OS lock until drop.
mod filesystem;
mod objects;
mod worktree;

pub use filesystem::FsBackend;
pub use knot_core::{Error, Result};
