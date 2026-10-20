// lib.rs

pub use clap::Parser;
pub use tracing::*;

pub use config::*;

mod config;
mod server;

pub use server::run;

// EOF
