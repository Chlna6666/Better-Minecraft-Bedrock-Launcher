//! High-level lazy world access built on top of the storage layer.
//!
//! World lifecycle, scanning, terrain access, and transactions currently share one cohesive access
//! implementation. Responsibility child modules are added only when code is physically split.

mod access;

pub use access::*;
