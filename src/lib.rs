//! Core library for nbspec: notebook-first OpenSpec orchestration.
//!
//! Exposes change operations behind a library boundary so that the CLI
//! binary and the planned MCP surface are both thin wrappers over the
//! same core functions.

pub mod cli;
pub mod operations;
