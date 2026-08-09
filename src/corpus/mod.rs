//! Run the existing differential pipeline over many projects in one pass.
//!
//! A manifest declares fully pinned projects, which are acquired into a
//! content-addressed cache before any analysis begins.
//!
//! Corpus entries are untrusted. Nothing acquired here is ever executed: no
//! install script runs, no package manager is invoked, and no project
//! configuration is evaluated. Entries are unpacked as data and read as data.

pub mod acquire;
pub mod archive;
pub mod cache;
pub mod entry;
pub mod integrity;
pub mod manifest;
