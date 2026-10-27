//! Shared native workflow core. CLI and MCP use the same validated commands.
pub mod args;
pub mod casefold;
pub mod config;
pub mod core;
pub mod external;
pub mod git;
pub mod lifecycle;
pub mod mcp;
pub mod review;
pub mod shipping;
pub mod store;
