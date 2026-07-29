pub mod adapters;
pub mod cli;
pub mod config;
pub mod discovery;
pub mod error;
pub mod matching;
pub mod model;
pub mod process;
pub mod reduce;
pub mod report;
pub mod scoring;

pub use error::{ConcordError, ErrorKind, Result};
