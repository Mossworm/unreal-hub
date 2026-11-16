use crate::ui::theme::Theme;
use gpui::{
    AnyElement, AnyView, App, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement,
    SharedString, Stateful, Window, div, prelude::*, rems, svg,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Check,
    ChevronDown,
    ChevronUp,
    Close,
    Cube,
    Folder,
    FolderOpen,
    Installs,
    Logo,
    More,
    Plus,
    Refresh,
    Search,
    Settings,
    Star,
    StarFilled,
    Warning,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
}

impl Icon {
    pub const ALL: [Icon; 21] = [
        Icon::Check,
        Icon::ChevronDown,
        Icon::ChevronUp,
        Icon::Close,
        Icon::Cube,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::Installs,
        Icon::Logo,
        Icon::More,
        Icon::Plus,
        Icon::Refresh,
        Icon::Search,
        Icon::Settings,
        Icon::Star,
        Icon::StarFilled,
        Icon::Warning,
        Icon::WindowClose,
        Icon::WindowMaximize,
        Icon::WindowMinimize,
        Icon::WindowRestore,
    ];

    pub fn path(self) -> &'static str {
        match self {
            Icon::Check => "icons/check.svg",
            Icon::ChevronDown => "icons/chevron_down.svg",
            Icon::ChevronUp => "icons/chevron_up.svg",
            Icon::Close => "icons/close.svg",
            Icon::Cube => "icons/cube.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::FolderOpen => "icons/folder_open.svg",
            Icon::Installs => "icons/installs.svg",
            Icon::Logo => "icons/logo.svg",
            Icon::More => "icons/more.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Search => "icons/search.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::Star => "icons/star.svg",
            Icon::StarFilled => "icons/star_filled.svg",
            Icon::Warning => "icons/warning.svg",
            Icon::WindowClose => "icons/window_close.svg",
            Icon::WindowMaximize => "icons/window_maximize.svg",
            Icon::WindowMinimize => "icons/window_minimize.svg",
            Icon::WindowRestore => "icons/window_restore.svg",
        }
    }
}

pub fn icon(icon: Icon, size_in_rems: f32, color: Hsla) -> impl IntoElement {
    svg()
        .path(icon.path())
        .flex_none()
        .size(rems(size_in_rems))
        .text_color(color)
}

pub struct TooltipView {
    text: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.elevated)
            .border_1()
            .border_color(theme.border_strong)
            .text_color(theme.text)
            .text_xs()
            .shadow_md()
            .child(self.text.clone())
    }
}

pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_window, cx| {
        let text = text.clone();
        cx.new(|_| TooltipView { text }).into()
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    leading_icon: Option<Icon>,
    trailing_icon: Option<Icon>,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::Secondary,
            leading_icon: None,
            trailing_icon: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }

    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    pub fn leading_icon(mut self, icon: Icon) -> Self {
        self.leading_icon = Some(icon);
        self
    }

    pub fn trailing_icon(mut self, icon: Icon) -> Self {
        self.trailing_icon = Some(icon);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(cx);
        let (background, hover_background, foreground, border) = match self.variant {
            ButtonVariant::Primary => (
                theme.accent,
                theme.accent_hover,
                theme.accent_text,
                theme.accent,
            ),
            ButtonVariant::Danger => (
                theme.danger,
                theme.danger_hover,
                theme.accent_text,
                theme.danger,
            ),
            ButtonVariant::Secondary => (
                theme.elevated,
                theme.selected,
                theme.text,
                theme.border_strong,
            ),
            ButtonVariant::Ghost => (
                gpui::transparent_black(),
                theme.hover,
                theme.text,
                gpui::transparent_black(),
            ),
        };
        let foreground = if self.disabled {
            theme.text_disabled
        } else {
            foreground
        };
        let disabled = self.disabled;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap_1p5()
            .h(rems(2.0))
            .px(rems(0.85))
            .rounded_md()
            .border_1()
            .border_color(if disabled { theme.border } else { border })
            .bg(if disabled && self.variant != ButtonVariant::Ghost {
                theme.surface
            } else {
                background
            })
            .text_color(foreground)
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when_some(self.leading_icon, |this, leading| {
                this.child(icon(leading, 0.95, foreground))
            })
            .child(self.label)
            .when_some(self.trailing_icon, |this, trailing| {
                this.child(icon(trailing, 0.85, foreground))
            })
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(hover_background))
                    .when_some(self.on_click, |this, handler| this.on_click(handler))
            })
    }
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: Icon,
    size: f32,
    disabled: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: Icon) -> Self {
        Self {
            id: id.into(),
            icon,
            size: 1.0,
            disabled: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(cx);
        let color = if self.disabled {
            theme.text_disabled
        } else {
            theme.text_muted
        };
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(rems(self.size + 0.9))
            .rounded_md()
            .child(icon(self.icon, self.size, color))
            .when_some(self.tooltip, |this, text| this.tooltip(tooltip(text)))
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(theme.hover))
                    .when_some(self.on_click, |this, handler| this.on_click(handler))
            })
    }
}

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    on: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self {
            id: id.into(),
            on,
            disabled: false,
            on_click: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(cx);
        let track = if self.on {
            theme.accent
        } else {
            theme.border_strong
        };
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .w(rems(2.25))
            .h(rems(1.25))
            .p(rems(0.125))
            .rounded_full()
            .bg(track)
            .when(self.on, |this| this.justify_end())
            .when(self.disabled, |this| this.opacity(0.4))
            .child(div().size(rems(1.0)).rounded_full().bg(gpui::white()))
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .when_some(self.on_click, |this, handler| this.on_click(handler))
            })
    }
}

#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    label: SharedString,
    detail: Option<SharedString>,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: bool, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            checked,
            label: label.into(),
            detail: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(cx);
        let box_color = if self.checked {
            theme.accent
        } else {
            theme.input
        };
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(if self.disabled {
                theme.text_disabled
            } else {
                theme.text
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(rems(1.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(if self.checked {
                        theme.accent
                    } else {
                        theme.border_strong
                    })
                    .bg(box_color)
                    .when(self.checked, |this| {
                        this.child(icon(Icon::Check, 0.75, theme.accent_text))
                    }),
            )
            .child(self.label)
            .when_some(self.detail, |this, detail| {
                this.child(div().text_color(theme.text_muted).child(detail))
            })
            .when(self.disabled, |this| this.opacity(0.6))
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .when_some(self.on_click, |this, handler| this.on_click(handler))
            })
    }
}

#[derive(IntoElement)]
pub struct Radio {
    id: ElementId,
    selected: bool,
    children: Vec<AnyElement>,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Radio {
    pub fn new(id: impl Into<ElementId>, selected: bool) -> Self {
        Self {
            id: id.into(),
            selected,
            children: Vec::new(),
            disabled: false,
            on_click: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Radio {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Radio {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .text_sm()
            .text_color(if self.disabled {
                theme.text_disabled
            } else {
                theme.text
            })
            .when(self.selected, |this| this.bg(theme.highlight))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(rems(1.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if self.selected {
                        theme.accent
                    } else {
                        theme.border_strong
                    })
                    .bg(theme.input)
                    .when(self.selected, |this| {
                        this.child(div().size(rems(0.5)).rounded_full().bg(theme.accent))
                    }),
            )
            .children(self.children)
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(theme.hover))
                    .when_some(self.on_click, |this, handler| this.on_click(handler))
            })
    }
}

pub fn badge(text: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(color.opacity(0.5))
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_xs()
        .whitespace_nowrap()
        .child(text.into())
}

pub fn running_badge(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(theme.success)
        .child(div().size(rems(0.45)).rounded_full().bg(theme.success))
        .child(tr!("실행 중", "Running"))
}

pub fn spinner_text(theme: &Theme, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .text_color(theme.text_muted)
        .child(icon(Icon::Refresh, 0.8, theme.text_muted))
        .child(text.into())
}

pub fn menu_container(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(rems(12.0))
        .p_1()
        .rounded_md()
        .bg(theme.elevated)
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
        .text_sm()
        .text_color(theme.text)
}

pub fn menu_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
    disabled_reason: Option<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let hover = theme.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .h(rems(1.9))
        .px_2()
        .rounded_sm()
        .whitespace_nowrap()
        .child(label.into())
        .when(disabled, |this| this.text_color(theme.text_disabled))
        .when_some(disabled_reason.filter(|_| disabled), |this, reason| {
            this.tooltip(tooltip(reason))
        })
        .when(!disabled, |this| {
            this.cursor_pointer().hover(move |style| style.bg(hover))
        })
}

pub fn menu_separator(theme: &Theme) -> impl IntoElement {
    div().my_1().h_px().bg(theme.border)
}

pub fn dialog_frame(
    theme: &Theme,
    title: impl Into<SharedString>,
    width_in_rems: f32,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_col()
        .w(rems(width_in_rems))
        .max_w_full()
        .max_h_full()
        .rounded_lg()
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
        .text_color(theme.text)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_4()
                .h(rems(3.0))
                .flex_none()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(
                    IconButton::new("dialog-close", Icon::Close)
                        .size(0.9)
                        .tooltip(tr!("닫기 (Esc)", "Close (Esc)"))
                        .on_click(on_close),
                ),
        )
}

pub fn dialog_body(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .text_sm()
        .text_color(theme.text)
}

pub fn dialog_footer(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap_2()
        .px_4()
        .py_3()
        .border_t_1()
        .border_color(theme.border)
}

pub fn warning_note(theme: &Theme, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .p_2()
        .rounded_md()
        .bg(theme.warning.opacity(0.1))
        .text_color(theme.warning)
        .text_sm()
        .child(icon(Icon::Warning, 1.0, theme.warning))
        .child(div().flex_1().min_w_0().child(text.into()))
}

pub fn error_note(theme: &Theme, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .p_2()
        .rounded_md()
        .bg(theme.danger.opacity(0.12))
        .text_color(theme.danger)
        .text_sm()
        .child(icon(Icon::Warning, 1.0, theme.danger))
        .child(div().flex_1().min_w_0().child(text.into()))
}

pub fn section_label(theme: &Theme, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
        .child(text.into())
}
