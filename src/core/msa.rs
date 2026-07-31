// This forwarding module preserves the original `managed::msa` module
// namespace after `bedrock_auth.rs` became a nested module of the local-account
// aggregation layer.
include!("bedrock_auth/msa.rs");
