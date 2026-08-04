pub mod cli;
pub mod click;
pub mod config;
pub mod doctor;
pub mod input;
pub mod runtime;
pub mod setup;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("{operation} ({path}): {source}")]
    Io {
        operation: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
