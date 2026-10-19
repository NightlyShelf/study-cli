//! Domain types, the JSON index, config and secrets shared by every crate.

pub mod config;
pub mod error;
pub mod index;
pub mod paths;
pub mod secrets;

pub use error::{Error, Result};
