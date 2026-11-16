use crate::i18n;
use crate::platform;
use crate::state::{HubEvent, HubState, LaunchOutcome, Toast, ToastKind, UpdateStatus};
use crate::storage::{Tab, WindowPlacement};
use crate::tray::{self, Tray, TrayCommand, TraySignal};
use crate::ui::build_dialog::BuildDialog;
use crate::ui::components::{Button, Icon, IconButton, icon};
use crate::ui::dialogs::{
    CacheCleanupDialog, ChangeEngineDialog, ChangeEngineMode, DialogEvent, RemoveEngineDialog,
};
use crate::ui::installs_view::{InstallsEvent, InstallsView};
use crate::ui::new_project_dialog::NewProjectDialog;
use crate::ui::projects_view::{ProjectsEvent, ProjectsView};
use crate::ui::settings_view::SettingsDialog;
use crate::ui::theme::Theme;
use crate::ui::{Confirm, Dismiss};
use crate::util::ResultExt;
use crate::util::format_bytes;
use futures::StreamExt as _;
use gpui::{
    AnyElement, AnyView, Context, Entity, ExternalPaths, FocusHandle, FontWeight, IntoElement,
    MouseButton, Render, Subscription, Task, Window, WindowBounds, WindowControlArea, div,
    prelude::*, px, rems,
};
use std::time::Duration;

const TITLEBAR_HEIGHT: f32 = 34.0;
const TOAST_DURATION: Duration = Duration::from_secs(4);

enum Modal {
    Settings(Entity<SettingsDialog>),
    ChangeEngine(Entity<ChangeEngineDialog>),
    CacheCleanup(Entity<CacheCleanupDialog>),
    RemoveEngine(Entity<RemoveEngineDialog>),
    NewProject(Entity<NewProjectDialog>),
    Build(Entity<BuildDialog>),
}

impl Modal {
    fn view(&self) -> AnyView {
        match self {
            Modal::Settings(view) => view.clone().into(),
            Modal::ChangeEngine(view) => view.clone().into(),
            Modal::CacheCleanup(view) => view.clone().into(),
            Modal::RemoveEngine(view) => view.clone().into(),
            Modal::NewProject(view) => view.clone().into(),
            Modal::Build(view) => view.clone().into(),
        }
    }

    fn confirm(&self, window: &mut Window, cx: &mut Context<HubView>) {
        match self {
            Modal::Settings(_) => {}
            Modal::ChangeEngine(view) => view.update(cx, |dialog, cx| dialog.confirm(window, cx)),
            Modal::CacheCleanup(view) => view.update(cx, |dialog, cx| dialog.confirm(window, cx)),
            Modal::RemoveEngine(view) => view.update(cx, |dialog, cx| dialog.confirm(window, cx)),
            Modal::NewProject(view) => view.update(cx, |dialog, cx| dialog.confirm(window, cx)),
            Modal::Build(view) => view.update(cx, |dialog, cx| dialog.confirm(window, cx)),
        }
    }

    fn request_close(&self, cx: &mut Context<HubView>) -> bool {
        match self {
            Modal::NewProject(view) => view.update(cx, |dialog, cx| dialog.request_close(cx)),
            _ => true,
        }
    }
}

struct ActiveToast {
    id: usize,
    toast: Toast,
}

pub struct HubView {
    state: Entity<HubState>,
    focus_handle: FocusHandle,
    tab: Tab,
    projects: Entity<ProjectsView>,
    installs: Entity<InstallsView>,
    modal: Option<Modal>,
    modal_subscription: Option<Subscription>,
    toasts: Vec<ActiveToast>,
    toast_tasks: Vec<(usize, Task<()>)>,
    next_toast_id: usize,
    placement_task: Option<Task<()>>,
    tray: Option<Tray>,
    _tray_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl HubView {
    pub fn new(
        state: Entity<HubState>,
        started_hidden: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let projects = cx.new(|cx| ProjectsView::new(state.clone(), window, cx));
        let installs = cx.new(|cx| InstallsView::new(state.clone(), cx));
        let tab = state.read(cx).settings.last_tab;
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        let subscriptions = vec![
            cx.subscribe_in(&state, window, Self::handle_hub_event),
            cx.subscribe_in(&projects, window, Self::handle_projects_event),
            cx.subscribe_in(&installs, window, Self::handle_installs_event),
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.apply_theme(window, cx);
                this.apply_language(window, cx);
                this.sync_tray(cx);
                cx.notify();
            }),
            cx.observe_window_appearance(window, |this, window, cx| this.apply_theme(window, cx)),
            cx.observe_window_bounds(window, |this, window, cx| {
                this.schedule_placement_save(window, cx)
            }),
        ];

        let mut tray_signals = tray::install_event_handlers();
        let tray_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(signal) = tray_signals.next().await {
                let handled = this.update_in(cx, |view, window, cx| {
                    view.handle_tray_signal(signal, window, cx)
                });
                if handled.is_err() {
                    break;
                }
            }
        });
        let close_view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let hide = close_view
                .update(cx, |view, cx| {
                    view.tray.is_some() && view.state.read(cx).settings.close_to_tray
                })
                .unwrap_or(false);
            if hide {
                platform::set_window_visible(window, false);
            }
            !hide
        });

        let mut view = Self {
            state: state.clone(),
            focus_handle,
            tab,
            projects,
            installs,
            modal: None,
            modal_subscription: None,
            toasts: Vec::new(),
            toast_tasks: Vec::new(),
            next_toast_id: 0,
            placement_task: None,
            tray: None,
            _tray_task: tray_task,
            _subscriptions: subscriptions,
        };
        view.sync_tray(cx);
        if started_hidden && view.tray.is_none() {
            platform::set_window_visible(window, true);
        }
        view.apply_theme(window, cx);
        let startup_toasts = state.update(cx, |state, _| state.take_startup_toasts());
        for toast in startup_toasts {
            view.push_toast(toast, cx);
        }
        if tab == Tab::Installs {
            state.update(cx, |state, cx| state.ensure_engine_sizes(cx));
        }
        view
    }

    fn apply_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = i18n::resolve(self.state.read(cx).settings.language);
        if language == i18n::current() {
            return;
        }
        i18n::set_language(language);
        if let Some(tray) = &mut self.tray {
            tray.refresh_labels().log_err();
        }
        window.refresh();
    }

    fn sync_tray(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let wanted = state.settings.shows_tray_icon();
        let recent = state.recent_projects(tray::RECENT_PROJECT_LIMIT);
        match (&mut self.tray, wanted) {
            (Some(existing), true) => {
                existing.update_recent(recent).log_err();
            }
            (Some(_), false) => self.tray = None,
            (None, true) => match Tray::new(recent) {
                Ok(created) => self.tray = Some(created),
                Err(error) => log::error!("Cannot create the tray icon: {error:#}"),
            },
            (None, false) => {}
        }
    }

    fn show_window(&self, window: &mut Window) {
        platform::set_window_visible(window, true);
        window.activate_window();
    }

    fn handle_tray_signal(
        &mut self,
        signal: TraySignal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self
            .tray
            .as_ref()
            .and_then(|tray| tray.command_for(&signal))
        else {
            return;
        };
        match command {
            TrayCommand::ShowWindow => self.show_window(window),
            TrayCommand::OpenProject(key) => {
                let outcome = self
                    .state
                    .update(cx, |state, cx| state.launch_project(&key, cx));
                if outcome == LaunchOutcome::EngineMissing {
                    self.show_window(window);
                    self.select_tab(Tab::Projects, cx);
                    self.open_change_engine(key, ChangeEngineMode::MissingEngine, window, cx);
                }
            }
            TrayCommand::Quit => cx.quit(),
        }
    }

    fn render_update_banner(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.state.read(cx).visible_update()?.clone();
        let (message, buttons): (String, Vec<AnyElement>) = match status {
            UpdateStatus::Idle => return None,
            UpdateStatus::Available(release) => (
                trf!(
                    "새 버전 v{}이(가) 있습니다",
                    "Version v{} is available",
                    release.version
                ),
                vec![
                    Button::new("update-start", tr!("업데이트", "Update"))
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.download_update(cx))
                        }))
                        .into_any_element(),
                    Button::new("update-later", tr!("나중에", "Later"))
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.dismiss_update(cx))
                        }))
                        .into_any_element(),
                ],
            ),
            UpdateStatus::Downloading {
                release,
                received,
                total,
            } => {
                let progress = match total.filter(|total| *total > 0) {
                    Some(total) => format!("{}%", received.saturating_mul(100) / total),
                    None => format_bytes(received),
                };
                (
                    trf!(
                        "v{} 내려받는 중… {progress}",
                        "Downloading v{}… {progress}",
                        release.version
                    ),
                    Vec::new(),
                )
            }
            UpdateStatus::Ready { release, .. } => (
                trf!(
                    "v{} 업데이트가 준비되었습니다",
                    "The v{} update is ready",
                    release.version
                ),
                vec![
                    Button::new(
                        "update-install",
                        tr!("재시작하여 업데이트", "Restart to Update"),
                    )
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.install_update(cx))
                    }))
                    .into_any_element(),
                ],
            ),
            UpdateStatus::Failed(error) => (
                trf!("업데이트 실패: {error}", "Update failed: {error}"),
                vec![
                    Button::new("update-retry", tr!("다시 시도", "Retry"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.dismiss_update(cx);
                                state.check_for_updates(true, cx);
                            })
                        }))
                        .into_any_element(),
                    Button::new("update-close", tr!("닫기", "Close"))
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.dismiss_update(cx))
                        }))
                        .into_any_element(),
                ],
            ),
        };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_3()
                .mx_5()
                .mt_4()
                .px_4()
                .py_2()
                .rounded_md()
                .bg(theme.highlight)
                .border_1()
                .border_color(theme.accent.opacity(0.4))
                .text_sm()
                .child(icon(Icon::Refresh, 1.0, theme.accent))
                .child(div().flex_1().min_w_0().child(message))
                .children(buttons)
                .into_any_element(),
        )
    }

    fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let preference = self.state.read(cx).settings.theme;
        let theme = Theme::resolve(preference, window.appearance());
        let changed = cx
            .try_global::<Theme>()
            .is_none_or(|current| current.is_dark != theme.is_dark);
        if changed {
            cx.set_global(theme);
            window.refresh();
        }
    }

    fn schedule_placement_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placement = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => WindowPlacement {
                x: bounds.origin.x.into(),
                y: bounds.origin.y.into(),
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
                maximized: false,
            },
            WindowBounds::Maximized(bounds) | WindowBounds::Fullscreen(bounds) => WindowPlacement {
                x: bounds.origin.x.into(),
                y: bounds.origin.y.into(),
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
                maximized: true,
            },
        };
        let state = self.state.clone();
        self.placement_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            state
                .update(cx, |state, cx| {
                    state.update_settings(cx, |settings| settings.window = Some(placement))
                })
                .log_err();
        }));
    }

    fn handle_hub_event(
        &mut self,
        _state: &Entity<HubState>,
        event: &HubEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HubEvent::Toast(toast) => self.push_toast(toast.clone(), cx),
            HubEvent::RevealProject(key) => {
                self.select_tab(Tab::Projects, cx);
                let key = key.clone();
                self.projects
                    .update(cx, |projects, cx| projects.reveal(&key, cx));
            }
            HubEvent::EditorLaunched => {
                if self.state.read(cx).settings.minimize_after_launch {
                    window.minimize_window();
                }
            }
        }
    }

    fn handle_projects_event(
        &mut self,
        _projects: &Entity<ProjectsView>,
        event: &ProjectsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectsEvent::ChangeEngine(key) => {
                self.open_change_engine(key.clone(), ChangeEngineMode::Change, window, cx)
            }
            ProjectsEvent::MissingEngine(key) => {
                self.open_change_engine(key.clone(), ChangeEngineMode::MissingEngine, window, cx)
            }
            ProjectsEvent::NewProject => {
                let state = self.state.clone();
                let dialog = cx.new(|cx| NewProjectDialog::new(state, cx));
                self.show_modal(Modal::NewProject(dialog.clone()), &dialog, window, cx);
            }
            ProjectsEvent::CleanCache(key) => {
                let state = self.state.clone();
                let key = key.clone();
                let dialog = cx.new(|cx| CacheCleanupDialog::new(state, key, cx));
                self.show_modal(Modal::CacheCleanup(dialog.clone()), &dialog, window, cx);
            }
            ProjectsEvent::Build(key) => {
                let state = self.state.clone();
                let key = key.clone();
                let dialog = cx.new(|cx| BuildDialog::new(state, key, cx));
                self.show_modal(Modal::Build(dialog.clone()), &dialog, window, cx);
            }
        }
    }

    fn handle_installs_event(
        &mut self,
        _installs: &Entity<InstallsView>,
        event: &InstallsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InstallsEvent::RemoveEngine(key) => {
                let state = self.state.clone();
                let key = key.clone();
                let dialog = cx.new(|cx| RemoveEngineDialog::new(state, key, cx));
                self.show_modal(Modal::RemoveEngine(dialog.clone()), &dialog, window, cx);
            }
        }
    }

    fn open_change_engine(
        &mut self,
        key: String,
        mode: ChangeEngineMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let dialog = cx.new(|cx| ChangeEngineDialog::new(state, key, mode, cx));
        self.show_modal(Modal::ChangeEngine(dialog.clone()), &dialog, window, cx);
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let dialog = cx.new(|cx| SettingsDialog::new(state, cx));
        self.show_modal(Modal::Settings(dialog.clone()), &dialog, window, cx);
    }

    fn show_modal<D: gpui::EventEmitter<DialogEvent>>(
        &mut self,
        modal: Modal,
        dialog: &Entity<D>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.projects
            .update(cx, |projects, cx| projects.dismiss_menus(cx));
        self.installs
            .update(cx, |installs, cx| installs.dismiss_menus(cx));
        self.modal_subscription = Some(cx.subscribe_in(
            dialog,
            window,
            |this, _, event: &DialogEvent, window, cx| match event {
                DialogEvent::Close => this.close_modal(window, cx),
                DialogEvent::ShowInstalls => {
                    this.close_modal(window, cx);
                    this.select_tab(Tab::Installs, cx);
                }
            },
        ));
        self.modal = Some(modal);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.modal_subscription = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn push_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(ActiveToast { id, toast });
        if self.toasts.len() > 4 {
            let overflow = self.toasts.len() - 4;
            self.toasts.drain(..overflow);
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            this.update(cx, |this, cx| this.dismiss_toast(id, cx))
                .log_err();
        });
        self.toast_tasks.push((id, task));
        cx.notify();
    }

    fn dismiss_toast(&mut self, id: usize, cx: &mut Context<Self>) {
        self.toasts.retain(|toast| toast.id != id);
        self.toast_tasks.retain(|(task_id, _)| *task_id != id);
        cx.notify();
    }

    fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.state.update(cx, |state, cx| {
            state.update_settings(cx, |settings| settings.last_tab = tab);
            if tab == Tab::Installs {
                state.ensure_engine_sizes(cx);
            }
        });
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(modal) = self.modal.as_ref() {
            if modal.request_close(cx) {
                self.close_modal(window, cx);
            }
            return;
        }
        let projects_dismissed = self
            .projects
            .update(cx, |projects, cx| projects.dismiss_menus(cx));
        let installs_dismissed = self
            .installs
            .update(cx, |installs, cx| installs.dismiss_menus(cx));
        if !projects_dismissed && !installs_dismissed {
            cx.propagate();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.modal.as_ref() {
            Some(modal) => modal.confirm(window, cx),
            None => cx.propagate(),
        }
    }

    fn render_titlebar(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let maximized = window.is_maximized();
        let caption_button =
            |id: &'static str, glyph: Icon, area: WindowControlArea, hover: gpui::Hsla| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(46.))
                    .h_full()
                    .window_control_area(area)
                    .hover(move |style| style.bg(hover))
                    .child(icon(glyph, 0.75, theme.text))
            };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(TITLEBAR_HEIGHT))
            .bg(theme.titlebar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("titlebar-drag")
                    .flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .pl_3()
                    .window_control_area(WindowControlArea::Drag)
                    .child(icon(Icon::Logo, 1.1, theme.accent))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child("Unreal Hub"),
                    ),
            )
            .child(
                div().px_2().child(
                    IconButton::new("open-settings", Icon::Settings)
                        .tooltip(tr!("설정", "Settings"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                        ),
                ),
            )
            .child(caption_button(
                "window-minimize",
                Icon::WindowMinimize,
                WindowControlArea::Min,
                theme.hover,
            ))
            .child(caption_button(
                "window-maximize",
                if maximized {
                    Icon::WindowRestore
                } else {
                    Icon::WindowMaximize
                },
                WindowControlArea::Max,
                theme.hover,
            ))
            .child(caption_button(
                "window-close",
                Icon::WindowClose,
                WindowControlArea::Close,
                theme.close_hover,
            ))
    }

    fn render_sidebar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, label: &'static str, item_icon: Icon, tab: Tab| {
            let selected = self.tab == tab;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2p5()
                .h(rems(2.25))
                .px_3()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .when(selected, |this| {
                    this.bg(theme.selected).font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |this| this.hover(|style| style.bg(theme.hover)))
                .child(icon(
                    item_icon,
                    1.0,
                    if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                ))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.select_tab(tab, cx)))
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .w(rems(12.5))
            .p_2()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(item(
                "tab-projects",
                tr!("프로젝트", "Projects"),
                Icon::Folder,
                Tab::Projects,
            ))
            .child(item(
                "tab-installs",
                tr!("설치", "Installs"),
                Icon::Installs,
                Tab::Installs,
            ))
    }

    fn render_toasts(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .bottom_4()
            .right_4()
            .flex()
            .flex_col()
            .gap_2()
            .items_end()
            .children(self.toasts.iter().map(|active| {
                let color = match active.toast.kind {
                    ToastKind::Info => theme.accent,
                    ToastKind::Success => theme.success,
                    ToastKind::Error => theme.danger,
                };
                let id = active.id;
                div()
                    .id(("toast", id))
                    .flex()
                    .items_center()
                    .gap_3()
                    .max_w(rems(28.))
                    .pl_3()
                    .pr_1()
                    .py_2()
                    .rounded_md()
                    .bg(theme.elevated)
                    .border_1()
                    .border_color(theme.border_strong)
                    .shadow_lg()
                    .text_sm()
                    .text_color(theme.text)
                    .child(div().w(rems(0.25)).h(rems(1.0)).rounded_full().bg(color))
                    .child(div().flex_1().child(active.toast.message.clone()))
                    .child(
                        IconButton::new(("toast-close", id), Icon::Close)
                            .size(0.75)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.dismiss_toast(id, cx)),
                            ),
                    )
            }))
    }
}

impl Render for HubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let scale = self.state.read(cx).settings.ui_scale_factor();
        window.set_rem_size(px(16.0 * scale));
        let content: AnyView = match self.tab {
            Tab::Projects => self.projects.clone().into(),
            Tab::Installs => self.installs.clone().into(),
        };
        let drop_highlight = theme.highlight;
        let update_banner = self.render_update_banner(&theme, cx);
        div()
            .id("hub")
            .key_context("Hub")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::confirm))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family("Segoe UI")
            .text_sm()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if window.focused(cx).is_none() {
                        window.focus(&this.focus_handle);
                    }
                }),
            )
            .child(self.render_titlebar(&theme, window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(&theme, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(update_banner)
                            .child(div().flex_1().min_h_0().child(content)),
                    ),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                this.select_tab(Tab::Projects, cx);
                this.state
                    .update(cx, |state, cx| state.add_project_paths(paths, cx));
            }))
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_highlight))
            .when_some(self.modal.as_ref().map(Modal::view), |this, view| {
                this.child(
                    div()
                        .id("modal-overlay")
                        .absolute()
                        .top(px(TITLEBAR_HEIGHT))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_6()
                        .bg(theme.overlay)
                        .occlude()
                        .child(view),
                )
            })
            .child(self.render_toasts(&theme, cx))
    }
}
