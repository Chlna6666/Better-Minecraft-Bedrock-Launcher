//! Classic SNBT: compounds, homogeneous lists, typed B/I/L arrays, numeric
//! suffixes, booleans and single/double quoted UTF-8 strings.
//!
//! Quoted strings escape their delimiter and backslash. Binary `ByteString`,
//! `ShortArray` and End tags have no standard SNBT representation and are rejected
//! on text output rather than silently changing their binary type.

mod parser;
mod scan;
mod writer;

pub use parser::parse;
pub use writer::to_string;
