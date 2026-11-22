//! Library surface of the 0721 resource service.
//!
//! The binary (`src/main.rs`) is a thin entry point over this crate, so
//! the router assembly stays testable with `oneshot` from integration
//! tests.

pub mod api;
pub mod auth;
pub mod blob;
pub mod blob_ipfs;
pub mod blob_s3;
pub mod blob_selection;
pub mod config;
pub mod error;
pub mod mcp;
pub mod media;
pub mod oauth;
pub mod pipeline;
pub mod routes;
pub mod rpc;
pub mod share;
pub mod state;
pub mod static_files;
pub mod ws_tickets;
