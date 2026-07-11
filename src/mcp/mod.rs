//! Model Context Protocol server wrapping the nbspec operations library.
//!
//! Exposes one MCP tool per CLI verb (`create`, `display`, `validate`,
//! `render`, `merge`) over stdio. The server resolves the project
//! notebook exactly once at startup, fail-fast on missing notebook, and
//! holds an `Arc<McpContext>` for tool calls.
//!
//! The MCP surface is a thin layer over the operations library; every
//! tool handler delegates to [`crate::operations`].

mod errors;
pub mod params;

pub mod server;

pub use server::{McpConfiguration, run};
