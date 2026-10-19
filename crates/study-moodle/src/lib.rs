//! Moodle web-service client: mobile-app login flow, REST calls and the
//! typed wrappers the index sync needs.

pub mod api;
pub mod client;
pub mod error;
pub mod launch;
pub mod upload;

pub use client::MoodleClient;
pub use error::{Error, Result};
