module Temporal.Domain.Lib

/// Must remain the LAST file in Temporal.Domain.fsproj: Fable's Rust target
/// makes the last compiled file the crate root, and daemon/crates/temporal-core
/// points its [lib] path at the generated Lib.rs.
let domainVersion = "0.1.0"
