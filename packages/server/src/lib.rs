//! Library surface of the 0721 resource service.
//!
//! The binary (`src/main.rs`) is a thin entry point over this crate, so
//! the router assembly stays testable with `oneshot` from integration
//! tests.

pub mod api;
pub mod auth;
pub mod blob;
pub mod config;
pub mod error;
pub mod media;
pub mod pipeline;
pub mod routes;
pub mod state;
