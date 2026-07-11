//! Core library for nbspec: notebook-first OpenSpec orchestration.
//!
//! Exposes change operations behind a library boundary so that the CLI
//! binary and the planned MCP surface are both thin wrappers over the
//! same core functions.

pub mod archives;
pub mod changes;
pub mod cli;
pub mod configuration;
pub mod grammar;
pub mod mcp;
pub mod merging;
pub mod operations;
pub mod provenance;
pub mod rendering;
pub mod reviews;
pub mod schemata;
pub mod validation;
pub mod worknotes;
