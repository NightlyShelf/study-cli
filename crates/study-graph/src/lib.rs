//! Microsoft Graph client for Teams channel files: device-code login, teams,
//! channels, files, delta sync, transfers and channel messages.
//!
//! Auth reuses the Microsoft Teams first-party client with the `.default`
//! scope (SPEC §5.2); it rides on undocumented preauthorization and can break.
//! That choice is settled: never cycle through other client ids or scopes.

pub mod api;
pub mod auth;
pub mod client;
pub mod error;

pub use client::GraphClient;
pub use error::{Error, Result};
