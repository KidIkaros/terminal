//! Terminal emulator library — exposes the parser, grid, PTY, renderer, and
//! supporting modules so that benchmarks and integration tools can drive them
//! in-process without linking against the winit binary.

pub mod clipboard;
pub mod config;
pub mod grid;
pub mod image;
pub mod ligatures;
pub mod mouse;
pub mod parser;
pub mod pty;
pub mod render;
pub mod search;
pub mod selection;
pub mod tab_bar;
pub mod tabs;
pub mod theme;

#[cfg(test)]
mod integration_tests;
