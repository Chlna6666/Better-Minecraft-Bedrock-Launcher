//! Read-only access to Minecraft Bedrock's uncompressed `BRArchive` v1 containers.
//!
//! Only the entry table is loaded at open time. Payloads remain arbitrary bytes:
//! this crate does not decode PNG, JSON or compiled MCB, extract files, create a
//! runtime, or mutate game resources. Unsupported versions fail explicitly.
//! Optional `fearless_simd` acceleration validates long resource names with
//! supported CPU instructions, while scalar decoding remains available.

mod archive;
mod error;
mod index;
mod name;

pub use archive::Archive;
pub use error::Error;
pub use index::{Entry, Index};

/// CPU instruction policy for container metadata decoding. Payload codecs are
/// independent. Neither policy creates an executor or changes format validation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Acceleration {
    /// Use supported SIMD for sufficiently long names when the `simd` feature
    /// is enabled; otherwise use the scalar path. No unsupported ISA is called.
    #[default]
    Auto,
    /// Use the portable scalar implementation, including for reproducible A/B
    /// comparisons and environments where accelerated execution is undesired.
    Scalar,
}
