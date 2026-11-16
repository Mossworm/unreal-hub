pub mod components;
pub mod dialogs;
pub mod hub_view;
pub mod installs_view;
pub mod new_project_dialog;
pub mod projects_view;
pub mod settings_view;
pub mod text_input;
pub mod theme;

use anyhow::Result;
use gpui::{App, AssetSource, KeyBinding, SharedString, actions};
use std::borrow::Cow;

actions!(
    hub,
    [
        /// Close the open dialog or menu.
        Dismiss,
        /// Run the primary action of the open dialog.
        Confirm,
    ]
);

pub struct Assets;

impl Assets {
    fn icon(path: &str) -> Option<&'static [u8]> {
        let bytes: &'static [u8] = match path {
            "icons/check.svg" => include_bytes!("../assets/icons/check.svg"),
            "icons/chevron_down.svg" => include_bytes!("../assets/icons/chevron_down.svg"),
            "icons/chevron_up.svg" => include_bytes!("../assets/icons/chevron_up.svg"),
            "icons/close.svg" => include_bytes!("../assets/icons/close.svg"),
            "icons/cube.svg" => include_bytes!("../assets/icons/cube.svg"),
            "icons/folder.svg" => include_bytes!("../assets/icons/folder.svg"),
            "icons/folder_open.svg" => include_bytes!("../assets/icons/folder_open.svg"),
            "icons/installs.svg" => include_bytes!("../assets/icons/installs.svg"),
            "icons/logo.svg" => include_bytes!("../assets/icons/logo.svg"),
            "icons/more.svg" => include_bytes!("../assets/icons/more.svg"),
            "icons/plus.svg" => include_bytes!("../assets/icons/plus.svg"),
            "icons/refresh.svg" => include_bytes!("../assets/icons/refresh.svg"),
            "icons/search.svg" => include_bytes!("../assets/icons/search.svg"),
            "icons/settings.svg" => include_bytes!("../assets/icons/settings.svg"),
            "icons/star.svg" => include_bytes!("../assets/icons/star.svg"),
            "icons/star_filled.svg" => include_bytes!("../assets/icons/star_filled.svg"),
            "icons/warning.svg" => include_bytes!("../assets/icons/warning.svg"),
            "icons/window_close.svg" => include_bytes!("../assets/icons/window_close.svg"),
            "icons/window_maximize.svg" => include_bytes!("../assets/icons/window_maximize.svg"),
            "icons/window_minimize.svg" => include_bytes!("../assets/icons/window_minimize.svg"),
            "icons/window_restore.svg" => include_bytes!("../assets/icons/window_restore.svg"),
            _ => return None,
        };
        Some(bytes)
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::icon(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(if path.is_empty() || path.starts_with("icons") {
            components::Icon::ALL
                .iter()
                .map(|icon| SharedString::from(icon.path()))
                .collect()
        } else {
            Vec::new()
        })
    }
}

pub fn init(cx: &mut App) {
    text_input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("Hub")),
        KeyBinding::new("enter", Confirm, Some("Hub")),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_embedded_svg() {
        for icon in components::Icon::ALL {
            assert!(Assets::icon(icon.path()).is_some(), "{}", icon.path());
        }
    }
}
