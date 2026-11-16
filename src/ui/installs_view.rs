use crate::engines::{Engine, EngineSource};
use crate::paths::display_path;
use crate::state::{EngineRemoval, EngineSizeState, HubState, ToastKind};
use crate::ui::components::{
    Button, Icon, IconButton, badge, icon, menu_container, menu_item, menu_separator, spinner_text,
};
use crate::ui::theme::Theme;
use crate::util::{ResultExt, format_bytes};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, PathPromptOptions, Pixels, Point, Render, Subscription, Window, anchored,
    deferred, div, prelude::*, px, rems,
};

pub enum InstallsEvent {
    RemoveEngine(String),
}

struct EngineMenu {
    key: String,
    position: Point<Pixels>,
}

pub struct InstallsView {
    state: Entity<HubState>,
    menu: Option<EngineMenu>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InstallsEvent> for InstallsView {}

impl InstallsView {
    pub fn new(state: Entity<HubState>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        Self {
            state,
            menu: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn dismiss_menus(&mut self, cx: &mut Context<Self>) -> bool {
        let had_menu = self.menu.take().is_some();
        if had_menu {
            cx.notify();
        }
        had_menu
    }

    fn pick_engine_location(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("엔진 폴더 선택".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let selected = match receiver.await {
                Ok(Ok(paths)) => paths.and_then(|paths| paths.into_iter().next()),
                Ok(Err(error)) => {
                    state
                        .update(cx, |state, cx| {
                            state.toast(format!("폴더 선택 실패: {error:#}"), ToastKind::Error, cx)
                        })
                        .log_err();
                    None
                }
                Err(_) => None,
            };
            if let Some(path) = selected {
                state
                    .update(cx, |state, cx| state.add_engine_location(path, cx))
                    .log_err();
            }
        })
        .detach();
    }

    fn open_menu(&mut self, key: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu = Some(EngineMenu { key, position });
        cx.notify();
    }

    fn render_engine(
        &self,
        index: usize,
        engine: &Engine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let size = if !engine.available {
            None
        } else {
            Some(match state.engine_size(&engine.key) {
                Some(EngineSizeState::Known(bytes)) => format_bytes(bytes).into_any_element(),
                Some(EngineSizeState::Calculating) => {
                    spinner_text(theme, "계산 중…").into_any_element()
                }
                None => div().child("—").into_any_element(),
            })
        };
        let source_color = match engine.source {
            EngineSource::Launcher => theme.accent,
            EngineSource::SourceBuild => theme.success,
            EngineSource::Manual => theme.text_muted,
        };
        let menu_open = self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.key == engine.key);
        let context_key = engine.key.clone();
        let button_key = engine.key.clone();
        div()
            .id(("engine-row", index))
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .h(rems(4.0))
            .border_b_1()
            .border_color(theme.border)
            .when(menu_open, |this| this.bg(theme.highlight))
            .when(!engine.available, |this| this.opacity(0.55))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(context_key.clone(), event.position, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(rems(2.5))
                    .rounded_md()
                    .bg(theme.elevated)
                    .border_1()
                    .border_color(theme.border)
                    .child(icon(Icon::Logo, 1.5, theme.text)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(engine.version_label()),
                            )
                            .child(badge(engine.source.label(), source_color))
                            .when(!engine.available, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(theme.warning)
                                        .child(icon(Icon::Warning, 0.8, theme.warning))
                                        .child("경로 없음"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(display_path(&engine.root)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(rems(8.5))
                    .text_sm()
                    .text_color(theme.text_muted)
                    .flex()
                    .justify_end()
                    .children(size),
            )
            .child(
                div()
                    .id(("engine-menu-button", index))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(rems(1.9))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_menu(button_key.clone(), event.position, cx);
                        }),
                    )
                    .child(icon(Icon::Settings, 1.0, theme.text_muted)),
            )
            .into_any_element()
    }

    fn render_menu(
        &self,
        menu: &EngineMenu,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let engine = state.engine(&menu.key)?.clone();
        let available = engine.available;
        let launch_key = engine.key.clone();
        let remove_key = engine.key.clone();
        let reveal_root = engine.root.clone();
        let menu_items = menu_container(theme)
            .id("engine-menu")
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                menu_item("engine-launch", "에디터 실행", !available, None, theme).when(
                    available,
                    |this| {
                        this.on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.state
                                .update(cx, |state, cx| state.launch_engine(&launch_key, cx));
                            cx.notify();
                        }))
                    },
                ),
            )
            .child(
                menu_item("engine-reveal", "탐색기에서 열기", !available, None, theme).when(
                    available,
                    |this| {
                        this.on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            cx.open_with_system(&reveal_root);
                            cx.notify();
                        }))
                    },
                ),
            )
            .child(menu_separator(theme))
            .child(if available {
                menu_item("engine-remove", "제거…", false, None, theme).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.menu = None;
                        cx.emit(InstallsEvent::RemoveEngine(remove_key.clone()));
                        cx.notify();
                    },
                ))
            } else {
                menu_item("engine-forget", "목록에서 제거", false, None, theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        let task = this.state.update(cx, |state, cx| {
                            state.remove_engine(&remove_key, EngineRemoval::UnregisterOnly, cx)
                        });
                        task.detach_and_log_err(cx);
                        cx.notify();
                    }),
                )
            });
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu_items),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_empty(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let refreshing = self.state.read(cx).engines_refreshing;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(icon(Icon::Installs, 3.5, theme.text_disabled))
            .child(
                div()
                    .mt_2()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if refreshing {
                        "엔진을 찾는 중…"
                    } else {
                        "감지된 엔진이 없습니다"
                    }),
            )
            .child(div().text_sm().text_color(theme.text_muted).child(
                "Epic Games Launcher로 엔진을 설치하거나, 이미 있는 엔진 폴더를 추가하세요.",
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_3()
                    .child(
                        Button::new("empty-add-engine", "위치 추가")
                            .leading_icon(Icon::Plus)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_engine_location(cx))),
                    )
                    .child(
                        Button::new("empty-open-launcher", "Epic Launcher 열기").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.open_epic_launcher(cx))
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for InstallsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let state = self.state.read(cx);
        let engines = state.engines.clone();
        let refreshing = state.engines_refreshing;
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_5()
            .pt_5()
            .pb_4()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("설치"),
            )
            .child(div().flex_1())
            .when(refreshing, |this| {
                this.child(spinner_text(&theme, "검색 중…"))
            })
            .child(
                IconButton::new("rescan-engines", Icon::Refresh)
                    .tooltip("다시 검색")
                    .disabled(refreshing)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.rescan_engines(cx))
                    })),
            )
            .child(
                Button::new("add-engine-location", "위치 추가")
                    .leading_icon(Icon::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.pick_engine_location(cx))),
            );
        let body: AnyElement = if engines.is_empty() {
            self.render_empty(&theme, cx)
        } else {
            div()
                .id("engine-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .mx_5()
                .mb_5()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface)
                .overflow_y_scroll()
                .children(
                    engines
                        .iter()
                        .enumerate()
                        .map(|(index, engine)| self.render_engine(index, engine, &theme, cx)),
                )
                .into_any_element()
        };
        let menu = self
            .menu
            .as_ref()
            .and_then(|menu| self.render_menu(menu, &theme, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.text)
            .child(header)
            .child(body)
            .children(menu)
    }
}
