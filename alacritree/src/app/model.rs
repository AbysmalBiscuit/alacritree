//! The data models and decision functions behind `AlacritreeApp`, split out
//! so they can be read and tested without the render pass around them. Nothing
//! here names an egui type: an item that paints belongs in the parent module.

use std::path::{Path, PathBuf};

use alacritty_terminal::tty::Shell;

use serde_json::{Value, json};

use crate::bindings::{BindingAction, KeyBinding, NamedAction};
use crate::command_palette::{self};
use crate::config::{FontConfig, SidebarFocus, UiFont, UiTheme};
use crate::path_style::PathStyle;
use crate::projects::{Project, Worktree};
use crate::session::{LiveState, SessionActivity, SessionId, SessionKind, TermSize};
use crate::sidebar_nav::{self, SidebarRow};
use crate::workspace::WorkspaceKey;
use crate::wsl::{self};
use crate::wsl_helper::{self, WslProbe};
use crate::{herdr, path_style};

use super::{ActionOrigin, HarnessMark, Managed, managed_tooltip};

#[cfg(test)]
mod tests {
    use super::super::focus::DeferredClose;
    use super::super::sidebar::{HerdrRowData, RowName, herdr_display_name};
    use super::*;
    use crate::command_palette::PaletteItem;
    use crate::config::AttachMode;
    use crate::test_util::titled_herdr_agent as titled;
}
