//! Platform-agnostic core for the Linux native computer-use daemon and relay.
//!
//! This crate is a Rust port of `computer-use-macos/Sources/ComputerUseCore`.
//! The v2 agent protocol, tool catalog, pixel coordinate contract, and
//! lease/snapshot/receipt semantics are identical to the macOS implementation;
//! only the native desktop bindings differ (see `grok-computer-use-daemon`).

pub mod args;
pub mod catalog;
pub mod geometry;
pub mod mcp;
pub mod models;
pub mod paths;
pub mod protocol;
pub mod runtime;

pub use models::*;
