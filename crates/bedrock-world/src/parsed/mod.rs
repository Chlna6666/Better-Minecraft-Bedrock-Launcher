//! Structured parsers layered above raw Bedrock world records.
//!
//! Parser output models and reports currently live with the parser because they share decoding
//! invariants. They can be split later when the child modules own their implementations.

mod parser;

pub use parser::*;
