//! The module tree behind the `alacritree` binary.
//!
//! A library target lets integration tests under `tests/` name crate types
//! directly instead of spawning the binary, and gives the allocation gate a
//! test binary of its own, where a `#[global_allocator]` affects nothing else.

#![warn(unreachable_pub)]

pub mod alloc_count;
pub mod app;
pub mod bindings;
pub mod builtin_font;
pub mod cli;
pub mod clipboard;
pub mod clipboard_image;
pub mod color_glyph;
pub mod colors;
pub mod command_ext;
pub mod command_palette;
pub mod config;
pub mod crash_log;
pub mod decoration_sprites;
pub mod diff_viewer;
pub mod digest;
pub mod dll_search;
pub mod doppler;
pub mod file_drop;
pub mod focus_priority;
pub mod fonts;
pub mod frame_log;
pub mod git_nav;
pub mod git_status;
pub mod glyph_cache;
pub mod gpu_timing;
pub mod grid_gl;
pub mod grid_instances;
pub mod herdr;
pub mod ime;
pub mod input;
pub mod ipc;
pub mod jobs;
pub mod links;
pub mod logdir;
pub mod logging;
pub mod mcp;
pub mod mouse;
pub mod multiplexer;
pub mod notify;
pub mod panel_filter;
pub mod paste;
pub mod path_style;
pub mod pending_spawn;
pub mod pr_query;
pub mod pr_status;
pub mod project_refresh;
pub mod projects;
#[cfg(windows)]
pub mod pty_rearm;
pub mod repaint;
pub mod row_label;
pub mod scratchpad;
pub mod session;
pub mod shell_decision;
pub mod shortcut;
pub mod sidebar_focus;
pub mod sidebar_nav;
pub mod stale_exe;
pub mod startup_log;
pub mod state;
pub mod terminal_view;
#[cfg(test)]
mod test_util;
pub mod tools;
pub mod upstream;
#[cfg(windows)]
pub mod win_session;
pub mod workspace;
pub mod worktree;
pub mod worktree_liveness;
pub mod wsl;
pub mod wsl_helper;

/// The timing reports inside unit tests read allocation counts too.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: alloc_count::CountingAllocator = alloc_count::CountingAllocator;
