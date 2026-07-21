//! Workspace stub replacing the real `iana-time-zone` crate via
//! `[patch.crates-io]`.
//!
//! The real crate links Apple's CoreFoundation framework on macOS, and the
//! release pipeline cross-links the macOS binaries with zig on Linux, where
//! only libSystem stubs exist — any `-framework` argument fails the link.
//! The sole consumer in the product is chrono (pulled in by reedline, whose
//! only local-time user is a default prompt clock the REPL does not use),
//! and chrono consults this crate only as a *fallback* after its primary
//! timezone sources (the `TZ` variable, `/etc/localtime`) and before its
//! final UTC default. Always answering "unknown" here keeps chrono on those
//! primary sources and keeps the shipping binary free of Apple frameworks.
//!
//! The stub must stay version- and feature-compatible with what chrono
//! requests, or cargo silently falls back to the real crate: the release
//! preflight in the justfile catches that by failing on any
//! framework-linking crate in the macOS graph.

use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum GetTimezoneError {
    FailedParsingString,
    IoError(std::io::Error),
    OsError,
}

impl fmt::Display for GetTimezoneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GetTimezoneError::FailedParsingString => write!(formatter, "GetTimezoneError::FailedParsingString"),
            GetTimezoneError::IoError(error) => write!(formatter, "GetTimezoneError::IoError({error})"),
            GetTimezoneError::OsError => write!(formatter, "GetTimezoneError::OsError"),
        }
    }
}

impl Error for GetTimezoneError {}

impl From<std::io::Error> for GetTimezoneError {
    fn from(error: std::io::Error) -> Self {
        GetTimezoneError::IoError(error)
    }
}

pub fn get_timezone() -> Result<String, GetTimezoneError> {
    Err(GetTimezoneError::OsError)
}
