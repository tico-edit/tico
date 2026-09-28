//! tico's own library crate: shared by the `tico` editor binary
//! (`src/main.rs`) and the `tcat` binary (`src/bin/tcat.rs`), which reuses
//! the config/theme/syntax-highlighting pieces to colorize `cat`-style
//! output without pulling in the editor itself.

pub mod app;
pub mod browser;
pub mod buffer;
pub mod cli;
pub mod config;
pub mod fileio;
pub mod help;
pub mod history;
pub mod justify;
pub mod keymap;
pub mod lockfile;
pub mod options;
pub mod syntax;
pub mod theme;
pub mod ui;
pub mod watch;
