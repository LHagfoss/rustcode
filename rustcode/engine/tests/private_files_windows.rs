//! Run the private-file tests without the engine's Unix-only library tests.
#![cfg(windows)]
#![allow(dead_code)]
#[path = "../src/daemon/private_files.rs"]
mod private_files;
