#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod lifecycle;
pub mod model;
pub mod protocol;
pub mod schedule;
#[cfg(unix)]
pub mod server;
pub mod store;

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonError {
    InvalidSchedule(String),
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
    Storage(String),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSchedule(message) => write!(formatter, "invalid schedule: {message}"),
            Self::InvalidInput(message) => write!(formatter, "invalid input: {message}"),
            Self::NotFound(message) => write!(formatter, "not found: {message}"),
            Self::Conflict(message) => write!(formatter, "conflict: {message}"),
            Self::Storage(message) => write!(formatter, "scheduler storage error: {message}"),
        }
    }
}

impl std::error::Error for DaemonError {}

pub type Result<T> = std::result::Result<T, DaemonError>;
