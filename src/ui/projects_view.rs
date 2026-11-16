use crate::engines::EngineResolution;
use crate::projects::{ProjectPresence, ProjectRow, SortColumn, visible_row_indices};
use crate::state::{HubState, LaunchOutcome, ToastKind};
use crate::ui::components::{
    Button, Icon, IconButton, badge, icon, menu_container, menu_item, menu_separator,
    running_badge, tooltip,
};
use crate::ui::text_input::{TextInput, TextInputEvent};
use crate::ui::theme::Theme;
use crate::util::ResultExt;
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, PathPromptOptions, Pixels, Point, Render, ScrollStrategy, SharedString,
    Subscription, Task, UniformListScrollHandle, Window, anchored, deferred, div, prelude::*, px,
    rems, uniform_list,
};
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

const ENGINE_COLUMN_WIDTH: f32 = 11.0;
const ICON_COLUMN_WIDTH: f32 = 2.75;

pub enum ProjectsEvent {
    NewProject,
    ChangeEngine(String),
    MissingEngine(String),
    CleanCache(String),
    Build(String),
}

struct RowMenu {
    key: String,
    position: Point<Pixels>,
}

struct RowView {
    row: ProjectRow,
    running: bool,
    engine_label: SharedString,
    engine_tooltip: Option<SharedString>,
    engine_warning: bool,
}

pub struct ProjectsView {
    state: Entity<HubState>,
    search: Entity<TextInput>,
    query: String,
    visible: Vec<usize>,
    scroll_handle: UniformListScrollHandle,
    row_menu: Option<RowMenu>,
    add_menu_open: bool,
    highlighted: Option<String>,
    highlight_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProjectsEvent> for ProjectsView {}

impl ProjectsView {
    pub fn new(state: Entity<HubState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            TextInput::new(
                || tr!("이름 또는 경로 검색", "Search by name or path").into(),
                cx,
            )
        });
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.recompute(cx)),
            cx.subscribe(
                &search,
                |this, search, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Changed => {
                        this.query = search.read(cx).text().to_string();
                        this.recompute(cx);
                    }
                },
            ),
        ];
        let mut view = Self {
            state,
            search,
            query: String::new(),
            visible: Vec::new(),
            scroll_handle: UniformListScrollHandle::new(),
            row_menu: None,
            add_menu_open: false,
            highlighted: None,
            highlight_task: None,
            _subscriptions: subscriptions,
        };
        view.recompute(cx);
        view
    }

    fn recompute(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        self.visible = visible_row_indices(&state.rows, state.settings.project_sort, &self.query);
        cx.notify();
    }

    pub fn dismiss_menus(&mut self, cx: &mut Context<Self>) -> bool {
        let had_menu = self.row_menu.is_some() || self.add_menu_open;
        self.row_menu = None;
        self.add_menu_open = false;
        if had_menu {
            cx.notify();
        }
        had_menu
    }

    pub fn reveal(&mut self, key: &str, cx: &mut Context<Self>) {
        let mut position = self.visible_position(key, cx);
        if position.is_none() && !self.query.is_empty() {
            self.search.update(cx, |search, cx| search.set_text("", cx));
            self.query.clear();
            self.recompute(cx);
            position = self.visible_position(key, cx);
        }
        if let Some(position) = position {
            self.scroll_handle
                .scroll_to_item(position, ScrollStrategy::Center);
        }
        self.highlighted = Some(key.to_string());
        self.highlight_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2500))
                .await;
            this.update(cx, |this, cx| {
                this.highlighted = None;
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    fn visible_position(&self, key: &str, cx: &Context<Self>) -> Option<usize> {
        let rows = &self.state.read(cx).rows;
        self.visible
            .iter()
            .position(|index| rows.get(*index).is_some_and(|row| row.key == key))
    }

    fn open_project(&mut self, key: &str, cx: &mut Context<Self>) {
        let outcome = self
            .state
            .update(cx, |state, cx| state.launch_project(key, cx));
        if outcome == LaunchOutcome::EngineMissing {
            cx.emit(ProjectsEvent::MissingEngine(key.to_string()));
        }
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.set_text("", cx));
    }

    fn pick_project_files(&mut self, cx: &mut Context<Self>) {
        self.add_menu_open = false;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(tr!("프로젝트 추가", "Add Project").into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let paths: Option<Vec<PathBuf>> = match receiver.await {
                Ok(Ok(paths)) => paths,
                Ok(Err(error)) => {
                    state
                        .update(cx, |state, cx| {
                            state.toast(
                                trf!(
                                    "파일 선택 실패: {error:#}",
                                    "File selection failed: {error:#}"
                                ),
                                ToastKind::Error,
                                cx,
                            )
                        })
                        .log_err();
                    None
                }
                Err(_) => None,
            };
            if let Some(paths) = paths.filter(|paths| !paths.is_empty()) {
                state
                    .update(cx, |state, cx| state.add_project_paths(paths, cx))
                    .log_err();
            }
        })
        .detach();
        cx.notify();
    }

    fn set_sort(&mut self, column: SortColumn, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.update_settings(cx, |settings| {
                settings.project_sort = settings.project_sort.cycle(column)
            })
        });
    }

    fn row_views(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<(usize, RowView)> {
        let state = self.state.read(cx);
        range
            .filter_map(|position| {
                let index = *self.visible.get(position)?;
                let row = state.rows.get(index)?.clone();
                let (engine_label, engine_tooltip, engine_warning) = match &row.resolution {
                    None => ("—".into(), None, false),
                    Some(EngineResolution::Matched(key)) => match state.engine(key) {
                        Some(engine) => (
                            engine.version_label().into(),
                            Some(engine.label().into()),
                            false,
                        ),
                        None => ("—".into(), None, false),
                    },
                    Some(EngineResolution::Embedded(key)) => match state.engine(key) {
                        Some(engine) => (
                            trf!("{} (내장)", "{} (built-in)", engine.version_label()).into(),
                            Some(
                                trf!(
                                    "엔진 트리 내부 프로젝트 · {}",
                                    "Project inside the engine tree · {}",
                                    engine.label()
                                )
                                .into(),
                            ),
                            false,
                        ),
                        None => (tr!("(내장)", "(built-in)").into(), None, false),
                    },
                    Some(EngineResolution::NotInstalled(version)) => (
                        version.clone().into(),
                        Some(
                            trf!(
                                "엔진 {version}이(가) 설치되어 있지 않습니다",
                                "Engine {version} is not installed"
                            )
                            .into(),
                        ),
                        true,
                    ),
                    Some(EngineResolution::UnknownBuild) => (
                        tr!("알 수 없는 빌드", "Unknown build").into(),
                        Some(
                            trf!(
                                "엔진 빌드 '{}'을(를) 찾을 수 없습니다",
                                "Engine build '{}' not found",
                                row.association.clone().unwrap_or_default()
                            )
                            .into(),
                        ),
                        true,
                    ),
                };
                let running = state.is_project_running(&row.key);
                Some((
                    position,
                    RowView {
                        row,
                        running,
                        engine_label,
                        engine_tooltip,
                        engine_warning,
                    },
                ))
            })
            .collect()
    }

    fn render_row(
        &self,
        position: usize,
        view: RowView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RowView {
            row,
            running,
            engine_label,
            engine_tooltip,
            engine_warning,
        } = view;
        let key = row.key.clone();
        let unreachable = row.presence == ProjectPresence::Unreachable;
        let highlighted = self.highlighted.as_deref() == Some(row.key.as_str());
        let menu_open = self
            .row_menu
            .as_ref()
            .is_some_and(|menu| menu.key == row.key);
        let star_key = key.clone();
        let menu_key = key.clone();
        let context_key = key.clone();
        div()
            .id(("project-row", position))
            .group("project-row")
            .w_full()
            .flex()
            .items_center()
            .h(rems(3.5))
            .px_1()
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .when(highlighted || menu_open, |this| this.bg(theme.highlight))
            .hover(|style| style.bg(theme.hover))
            .when(unreachable, |this| this.opacity(0.55))
            .on_click(cx.listener(move |this, _, _, cx| this.open_project(&key, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.add_menu_open = false;
                    this.row_menu = Some(RowMenu {
                        key: context_key.clone(),
                        position: event.position,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id(("project-star", position))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(rems(ICON_COLUMN_WIDTH))
                    .h_full()
                    .tooltip(tooltip(if row.favorite {
                        tr!("즐겨찾기 해제", "Remove from favorites")
                    } else {
                        tr!("즐겨찾기", "Add to favorites")
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.state
                                .update(cx, |state, cx| state.toggle_favorite(&star_key, cx));
                        }),
                    )
                    .child(if row.favorite {
                        icon(Icon::StarFilled, 1.0, theme.warning).into_any_element()
                    } else {
                        icon(Icon::Star, 1.0, theme.text_disabled).into_any_element()
                    }),
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
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(row.name.clone()),
                            )
                            .when(row.is_cpp, |this| {
                                this.child(badge("C++", theme.text_muted))
                            })
                            .when(running, |this| this.child(running_badge(theme)))
                            .when(unreachable, |this| {
                                this.child(badge(
                                    tr!("드라이브 연결 안 됨", "Drive not connected"),
                                    theme.warning,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(row.folder.clone()),
                    ),
            )
            .child(
                div()
                    .id(("project-engine", position))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .w(rems(ENGINE_COLUMN_WIDTH))
                    .px_2()
                    .text_sm()
                    .text_color(if engine_warning {
                        theme.warning
                    } else {
                        theme.text
                    })
                    .child(div().truncate().child(engine_label))
                    .when(engine_warning, |this| {
                        this.child(icon(Icon::Warning, 0.9, theme.warning))
                    })
                    .when_some(engine_tooltip, |this, text| this.tooltip(tooltip(text))),
            )
            .child(
                div()
                    .id(("project-menu", position))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(rems(ICON_COLUMN_WIDTH))
                    .h_full()
                    .child(
                        div()
                            .id(("project-menu-button", position))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(rems(1.9))
                            .rounded_md()
                            .when(!menu_open, |this| this.invisible())
                            .group_hover("project-row", |style| style.visible())
                            .hover(|style| style.bg(theme.active))
                            .tooltip(tooltip(tr!("더 보기", "More")))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.add_menu_open = false;
                                    this.row_menu = Some(RowMenu {
                                        key: menu_key.clone(),
                                        position: event.position,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(icon(Icon::More, 1.0, theme.text_muted)),
                    ),
            )
            .into_any_element()
    }

    fn render_row_menu(
        &self,
        menu: &RowMenu,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let row = state.row(&menu.key)?.clone();
        let running = state.is_project_running(&row.key);
        let unavailable = row.presence != ProjectPresence::Present;
        let running_reason: Option<SharedString> = Some(
            tr!(
                "프로젝트가 실행 중일 때는 사용할 수 없습니다",
                "Unavailable while the project is running"
            )
            .into(),
        );
        let engine_missing = row
            .resolution
            .as_ref()
            .and_then(|resolution| resolution.engine_key())
            .and_then(|engine_key| state.engine(engine_key))
            .is_none();
        let build_reason: Option<SharedString> = if running {
            running_reason.clone()
        } else if engine_missing {
            Some(
                tr!(
                    "프로젝트의 엔진을 찾을 수 없습니다",
                    "The project's engine was not found"
                )
                .into(),
            )
        } else {
            None
        };
        let build_disabled = running || unavailable || engine_missing;
        let is_cpp = row.is_cpp;
        let reveal_path = row.path.clone();
        let change_key = row.key.clone();
        let clean_key = row.key.clone();
        let build_key = row.key.clone();
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        menu_container(theme)
                            .id("project-row-menu")
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.row_menu = None;
                                cx.notify();
                            }))
                            .child(
                                menu_item(
                                    "menu-reveal",
                                    tr!("탐색기에서 보기", "Show in Explorer"),
                                    unavailable,
                                    None,
                                    theme,
                                )
                                .when(!unavailable, |this| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.row_menu = None;
                                        cx.reveal_path(&reveal_path);
                                        cx.notify();
                                    }))
                                }),
                            )
                            .child(
                                menu_item(
                                    "menu-change-engine",
                                    tr!("엔진 버전 변경…", "Change Engine Version…"),
                                    running || unavailable,
                                    running_reason.clone().filter(|_| running),
                                    theme,
                                )
                                .when(
                                    !running && !unavailable,
                                    |this| {
                                        this.on_click(cx.listener(move |this, _, _, cx| {
                                            this.row_menu = None;
                                            cx.emit(ProjectsEvent::ChangeEngine(
                                                change_key.clone(),
                                            ));
                                            cx.notify();
                                        }))
                                    },
                                ),
                            )
                            .child(
                                menu_item(
                                    "menu-clean-cache",
                                    tr!("캐시 정리…", "Clean Cache…"),
                                    running || unavailable,
                                    running_reason.filter(|_| running),
                                    theme,
                                )
                                .when(
                                    !running && !unavailable,
                                    |this| {
                                        this.on_click(cx.listener(move |this, _, _, cx| {
                                            this.row_menu = None;
                                            cx.emit(ProjectsEvent::CleanCache(clean_key.clone()));
                                            cx.notify();
                                        }))
                                    },
                                ),
                            )
                            .when(is_cpp, |this| {
                                this.child(menu_separator(theme)).child(
                                    menu_item(
                                        "menu-build",
                                        tr!("빌드…", "Build…"),
                                        build_disabled,
                                        build_reason,
                                        theme,
                                    )
                                    .when(
                                        !build_disabled,
                                        |this| {
                                            this.on_click(cx.listener(move |this, _, _, cx| {
                                                this.row_menu = None;
                                                cx.emit(ProjectsEvent::Build(build_key.clone()));
                                                cx.notify();
                                            }))
                                        },
                                    ),
                                )
                            }),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = !self.query.is_empty();
        div()
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
                    .text_color(theme.text)
                    .child(tr!("프로젝트", "Projects")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .w(rems(17.0))
                    .child(icon(Icon::Search, 0.95, theme.text_muted))
                    .child(self.search.clone())
                    .when(has_query, |this| {
                        this.child(
                            IconButton::new("clear-search", Icon::Close)
                                .size(0.8)
                                .tooltip(tr!("검색 지우기", "Clear search"))
                                .on_click(cx.listener(|this, _, _, cx| this.clear_search(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .child(
                        Button::new("add-project", tr!("추가", "Add"))
                            .trailing_icon(Icon::ChevronDown)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.row_menu = None;
                                this.add_menu_open = !this.add_menu_open;
                                cx.notify();
                            })),
                    )
                    .when(self.add_menu_open, |this| {
                        this.child(
                            div().absolute().top(rems(2.25)).right_0().child(
                                deferred(
                                    menu_container(theme)
                                        .id("add-menu")
                                        .occlude()
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.add_menu_open = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            menu_item(
                                                "menu-add-from-disk",
                                                tr!(
                                                    "디스크에서 프로젝트 추가",
                                                    "Add project from disk"
                                                ),
                                                false,
                                                None,
                                                theme,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.pick_project_files(cx)
                                                }),
                                            ),
                                        ),
                                )
                                .with_priority(1),
                            ),
                        )
                    }),
            )
            .child(self.new_project_button("new-project", cx))
    }

    fn render_column_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let sort = self.state.read(cx).settings.project_sort;
        let header_cell = |id: &'static str, label: &'static str, column: SortColumn| {
            let arrow = (sort.column == Some(column)).then_some(if sort.descending {
                Icon::ChevronDown
            } else {
                Icon::ChevronUp
            });
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h_full()
                .cursor_pointer()
                .hover(|style| style.text_color(theme.text))
                .child(label)
                .when_some(arrow, |this, arrow| {
                    this.child(icon(arrow, 0.8, theme.text))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_sort(column, cx)))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(rems(2.25))
            .px_1()
            .border_b_1()
            .border_color(theme.border)
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_center()
                    .w(rems(ICON_COLUMN_WIDTH))
                    .child(icon(Icon::Star, 0.85, theme.text_muted)),
            )
            .child(div().flex().flex_1().h_full().child(header_cell(
                "sort-name",
                tr!("이름", "Name"),
                SortColumn::Name,
            )))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h_full()
                    .w(rems(ENGINE_COLUMN_WIDTH))
                    .child(header_cell(
                        "sort-engine",
                        tr!("엔진 버전", "Engine version"),
                        SortColumn::EngineVersion,
                    )),
            )
            .child(div().flex_none().w(rems(ICON_COLUMN_WIDTH)))
    }

    fn render_empty(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if !self.query.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(theme.text_muted)
                .child(trf!(
                    "'{}'와(과) 일치하는 프로젝트가 없습니다",
                    "No projects match '{}'",
                    self.query
                ))
                .child(
                    Button::new("empty-clear-search", tr!("검색 지우기", "Clear search"))
                        .on_click(cx.listener(|this, _, _, cx| this.clear_search(cx))),
                )
                .into_any_element();
        }
        let refreshing = self.state.read(cx).projects_refreshing;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(icon(Icon::Cube, 3.5, theme.text_disabled))
            .child(
                div()
                    .mt_2()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(if refreshing {
                        tr!("프로젝트를 찾는 중…", "Looking for projects…")
                    } else {
                        tr!("프로젝트 없음", "No Projects")
                    }),
            )
            .child(div().text_sm().text_color(theme.text_muted).child(tr!(
                "시작하려면 새 프로젝트를 생성하거나 프로젝트를 추가하세요.",
                "Create a new project or add an existing one to get started."
            )))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_3()
                    .child(
                        Button::new("empty-add-project", tr!("프로젝트 추가", "Add Project"))
                            .on_click(cx.listener(|this, _, _, cx| this.pick_project_files(cx))),
                    )
                    .child(self.new_project_button("empty-new-project", cx)),
            )
            .into_any_element()
    }

    fn new_project_button(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id, tr!("새 프로젝트", "New Project"))
            .primary()
            .leading_icon(Icon::Plus)
            .on_click(cx.listener(|this, _, _, cx| {
                this.dismiss_menus(cx);
                cx.emit(ProjectsEvent::NewProject);
            }))
    }
}

impl Render for ProjectsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let row_count = self.visible.len();
        let list: AnyElement = if row_count == 0 {
            self.render_empty(&theme, cx)
        } else {
            div()
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
                .overflow_hidden()
                .child(self.render_column_header(&theme, cx))
                .child(
                    uniform_list(
                        "project-list",
                        row_count,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            let theme = Theme::of(cx);
                            this.row_views(range, cx)
                                .into_iter()
                                .map(|(position, view)| this.render_row(position, view, &theme, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.scroll_handle.clone())
                    .flex_1(),
                )
                .into_any_element()
        };
        let row_menu = self
            .row_menu
            .as_ref()
            .and_then(|menu| self.render_row_menu(menu, &theme, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_header(&theme, cx))
            .child(list)
            .children(row_menu)
    }
}
