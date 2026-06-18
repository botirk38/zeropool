//! Shared helpers used by the Criterion bench suite.
//!
//! Keeping this in one module avoids each bench redefining identical
//! constants, touch loops, and competitor setups.

#![allow(dead_code)]

pub mod sizes;
pub mod touch;
pub mod competitors;
