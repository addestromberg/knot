//! Filesystem adapter. Opening a backend holds an exclusive OS lock until drop.
mod filesystem;
mod objects;
mod peers;
mod sync;
mod worktree;

pub use filesystem::FsBackend;
pub use knot_core::{Error, Result};
pub use peers::*;
pub use sync::*;
