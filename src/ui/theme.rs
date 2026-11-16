use crate::storage::ThemePreference;
use gpui::{App, Global, Hsla, WindowAppearance, rgb, rgba};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub is_dark: bool,
    pub background: Hsla,
    pub titlebar: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub elevated: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_disabled: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_text: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub overlay: Hsla,
    pub input: Hsla,
    pub focus: Hsla,
    pub selection: Hsla,
    pub highlight: Hsla,
    pub close_hover: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            background: rgb(0x1f1f1f).into(),
            titlebar: rgb(0x181818).into(),
            sidebar: rgb(0x181818).into(),
            surface: rgb(0x262626).into(),
            elevated: rgb(0x2c2c2c).into(),
            border: rgb(0x333333).into(),
            border_strong: rgb(0x474747).into(),
            text: rgb(0xe8e8e8).into(),
            text_muted: rgb(0x9c9c9c).into(),
            text_disabled: rgb(0x5f5f5f).into(),
            hover: rgba(0xffffff12).into(),
            active: rgba(0xffffff1f).into(),
            selected: rgb(0x343434).into(),
            accent: rgb(0x2f7de1).into(),
            accent_hover: rgb(0x4a8ff0).into(),
            accent_text: rgb(0xffffff).into(),
            danger: rgb(0xd9453f).into(),
            danger_hover: rgb(0xe65a54).into(),
            warning: rgb(0xe8a93a).into(),
            success: rgb(0x46b37a).into(),
            overlay: rgba(0x0000008c).into(),
            input: rgb(0x161616).into(),
            focus: rgb(0x2f7de1).into(),
            selection: rgba(0x2f7de166).into(),
            highlight: rgba(0x2f7de12e).into(),
            close_hover: rgb(0xc42b1c).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            background: rgb(0xf7f7f7).into(),
            titlebar: rgb(0xe9e9e9).into(),
            sidebar: rgb(0xeeeeee).into(),
            surface: rgb(0xffffff).into(),
            elevated: rgb(0xffffff).into(),
            border: rgb(0xdedede).into(),
            border_strong: rgb(0xc4c4c4).into(),
            text: rgb(0x1d1d1d).into(),
            text_muted: rgb(0x6b6b6b).into(),
            text_disabled: rgb(0xadadad).into(),
            hover: rgba(0x0000000d).into(),
            active: rgba(0x00000017).into(),
            selected: rgb(0xdcdcdc).into(),
            accent: rgb(0x1f6fd6).into(),
            accent_hover: rgb(0x3a82e2).into(),
            accent_text: rgb(0xffffff).into(),
            danger: rgb(0xc93a34).into(),
            danger_hover: rgb(0xd9524c).into(),
            warning: rgb(0xb7790f).into(),
            success: rgb(0x2a8a57).into(),
            overlay: rgba(0x00000059).into(),
            input: rgb(0xffffff).into(),
            focus: rgb(0x1f6fd6).into(),
            selection: rgba(0x1f6fd64d).into(),
            highlight: rgba(0x1f6fd61f).into(),
            close_hover: rgb(0xc42b1c).into(),
        }
    }

    pub fn resolve(preference: ThemePreference, appearance: WindowAppearance) -> Self {
        let dark = match preference {
            ThemePreference::Dark => true,
            ThemePreference::Light => false,
            ThemePreference::System => matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        };
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn of(cx: &App) -> Self {
        cx.try_global::<Theme>()
            .copied()
            .unwrap_or_else(Theme::dark)
    }
}
