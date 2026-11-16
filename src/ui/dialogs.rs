use crate::engines::{Engine, EngineSource};
use crate::projects::{
    CacheFolder, CleanupOptions, DeleteMode, cache_folder_targets, delete_paths, measure_paths,
    plugin_build_folders,
};
use crate::state::{EngineRemoval, HubState, ToastKind};
use crate::ui::components::{
    Button, Checkbox, Radio, dialog_body, dialog_footer, dialog_frame, error_note, section_label,
    spinner_text, warning_note,
};
use crate::ui::text_input::{TextInput, TextInputEvent};
use crate::ui::theme::Theme;
use crate::unreal::{AssociationKind, EngineVersion, classify_association};
use crate::util::{ResultExt, format_bytes};
use anyhow::Result;
use gpui::{
    AnyElement, Context, Entity, EventEmitter, Focusable as _, FontWeight, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, prelude::*, rems,
};
use std::path::PathBuf;

pub enum DialogEvent {
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeEngineMode {
    Change,
    MissingEngine,
}

pub struct ChangeEngineDialog {
    state: Entity<HubState>,
    project_key: String,
    project_name: String,
    mode: ChangeEngineMode,
    current_label: String,
    current_version: Option<EngineVersion>,
    current_engine_key: Option<String>,
    selected: Option<String>,
    create_backup: bool,
    task: Option<Task<()>>,
}

impl EventEmitter<DialogEvent> for ChangeEngineDialog {}

impl ChangeEngineDialog {
    pub fn new(
        state: Entity<HubState>,
        project_key: String,
        mode: ChangeEngineMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let hub = state.read(cx);
        let row = hub.row(&project_key).cloned();
        let current_engine_key = row
            .as_ref()
            .and_then(|row| row.resolution.as_ref())
            .and_then(|resolution| resolution.engine_key())
            .map(str::to_string);
        let current_engine = current_engine_key
            .as_deref()
            .and_then(|key| hub.engine(key));
        let association = row
            .as_ref()
            .and_then(|row| row.association.clone())
            .unwrap_or_default();
        let association_version = match classify_association(&association) {
            AssociationKind::Version { major, minor } => Some(EngineVersion {
                major,
                minor,
                patch: 0,
            }),
            _ => None,
        };
        let current_version = current_engine
            .and_then(|engine| engine.version)
            .or(association_version);
        let current_label = match current_engine {
            Some(engine) => engine.label(),
            None if association.is_empty() => "알 수 없음".to_string(),
            None => match classify_association(&association) {
                AssociationKind::Version { .. } => association.clone(),
                _ => format!("알 수 없는 빌드 ({association})"),
            },
        };
        let selected = current_engine_key
            .clone()
            .filter(|_| mode == ChangeEngineMode::Change)
            .or_else(|| {
                hub.installed_engines()
                    .find(|engine| {
                        association_version.is_some_and(|wanted| {
                            engine.version.is_some_and(|version| {
                                version.major == wanted.major && version.minor > wanted.minor
                                    || version.major > wanted.major
                            })
                        })
                    })
                    .or_else(|| hub.default_engine())
                    .map(|engine| engine.key.clone())
            });
        let project_name = row.map(|row| row.name).unwrap_or_default();
        Self {
            state,
            project_key,
            project_name,
            mode,
            current_label,
            current_version,
            current_engine_key,
            selected,
            create_backup: true,
            task: None,
        }
    }

    fn selected_engine<'a>(&self, engines: &'a [Engine]) -> Option<&'a Engine> {
        self.selected
            .as_deref()
            .and_then(|key| engines.iter().find(|engine| engine.key == key))
    }

    fn can_submit(&self) -> bool {
        self.task.is_none()
            && self.selected.is_some()
            && (self.mode == ChangeEngineMode::MissingEngine
                || self.selected != self.current_engine_key)
    }

    pub fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let launch_after = self.mode == ChangeEngineMode::MissingEngine;
        self.submit(launch_after, cx);
    }

    fn submit(&mut self, launch_after: bool, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        let Some(engine_key) = self.selected.clone() else {
            return;
        };
        let project_key = self.project_key.clone();
        let create_backup = self.create_backup;
        let change = self.state.update(cx, |state, cx| {
            state.change_project_engine(&project_key, &engine_key, create_backup, launch_after, cx)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let succeeded = change.await.is_ok();
            this.update(cx, |dialog, cx| {
                dialog.task = None;
                if succeeded {
                    cx.emit(DialogEvent::Close);
                }
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }
}

impl Render for ChangeEngineDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let engines: Vec<Engine> = self.state.read(cx).installed_engines().cloned().collect();
        let selected_version = self
            .selected_engine(&engines)
            .and_then(|engine| engine.version);
        let busy = self.task.is_some();
        let can_submit = self.can_submit();
        let version_warning = match (self.current_version, selected_version) {
            (Some(current), Some(selected))
                if (selected.major, selected.minor) > (current.major, current.minor) =>
            {
                Some(
                    "상위 버전으로 바꾸면 에셋이 변환되어 이전 버전에서 열 수 없게 될 수 있습니다.",
                )
            }
            (Some(current), Some(selected))
                if (selected.major, selected.minor) < (current.major, current.minor) =>
            {
                Some("하위 버전으로는 정상적으로 열리지 않을 수 있습니다.")
            }
            _ => None,
        };
        let title = match self.mode {
            ChangeEngineMode::Change => "엔진 버전 변경",
            ChangeEngineMode::MissingEngine => "엔진을 찾을 수 없음",
        };
        let intro: AnyElement = match self.mode {
            ChangeEngineMode::Change => div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.project_name.clone()),
                )
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .child(format!("현재: {}", self.current_label)),
                )
                .into_any_element(),
            ChangeEngineMode::MissingEngine => div()
                .child(format!(
                    "이 프로젝트({})는 엔진 {}을(를) 사용하지만 설치되어 있지 않습니다.",
                    self.project_name, self.current_label
                ))
                .into_any_element(),
        };
        let engine_list: AnyElement = if engines.is_empty() {
            error_note(
                &theme,
                "설치된 엔진이 없습니다. 설치 탭에서 엔진을 추가하세요.",
            )
            .into_any_element()
        } else {
            div()
                .id("engine-choices")
                .flex()
                .flex_col()
                .gap_0p5()
                .max_h(rems(16.0))
                .overflow_y_scroll()
                .children(engines.iter().enumerate().map(|(index, engine)| {
                    let key = engine.key.clone();
                    Radio::new(
                        ("engine-choice", index),
                        self.selected.as_deref() == Some(engine.key.as_str()),
                    )
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(key.clone());
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(rems(4.5))
                            .font_weight(FontWeight::MEDIUM)
                            .child(engine.version_label()),
                    )
                    .child(
                        div()
                            .w(rems(5.0))
                            .text_color(theme.text_muted)
                            .child(engine.source.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .when(engine.source != EngineSource::Launcher, |this| {
                                this.child(crate::paths::display_path(&engine.root))
                            }),
                    )
                }))
                .into_any_element()
        };
        let footer = match self.mode {
            ChangeEngineMode::Change => dialog_footer(&theme)
                .child(
                    Button::new("change-cancel", "취소")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                )
                .child(
                    Button::new("change-apply", "변경")
                        .disabled(!can_submit)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(false, cx))),
                )
                .child(
                    Button::new(
                        "change-apply-open",
                        if busy {
                            "변경 중…"
                        } else {
                            "변경 후 열기"
                        },
                    )
                    .primary()
                    .disabled(!can_submit)
                    .on_click(cx.listener(|this, _, _, cx| this.submit(true, cx))),
                ),
            ChangeEngineMode::MissingEngine => dialog_footer(&theme)
                .child(
                    Button::new("missing-open-launcher", "Epic Launcher 열기")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.open_epic_launcher(cx))
                        })),
                )
                .child(div().flex_1())
                .child(
                    Button::new("missing-cancel", "취소")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                )
                .child(
                    Button::new("missing-open", if busy { "여는 중…" } else { "열기" })
                        .primary()
                        .disabled(!can_submit)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(true, cx))),
                ),
        };
        dialog_frame(
            &theme,
            title,
            34.0,
            cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
        )
        .child(
            dialog_body(&theme)
                .child(intro)
                .when(self.mode == ChangeEngineMode::MissingEngine, |this| {
                    this.child(section_label(&theme, "다른 버전으로 열기"))
                })
                .child(engine_list)
                .when_some(version_warning, |this, warning| {
                    this.child(warning_note(&theme, warning))
                })
                .child(
                    Checkbox::new("create-backup", self.create_backup, ".uproject 백업 만들기")
                        .detail("(.uproject.bak)")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.create_backup = !this.create_backup;
                            cx.notify();
                        })),
                ),
        )
        .child(footer)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CacheSizes {
    intermediate: Option<u64>,
    derived_data_cache: Option<u64>,
    saved: Option<u64>,
    saved_config: u64,
    binaries: Option<u64>,
    plugins: u64,
}

impl CacheSizes {
    fn measure(project_dir: &std::path::Path) -> Self {
        let measure_folder = |name: &str| {
            let path = project_dir.join(name);
            path.is_dir().then(|| measure_paths(&[path]))
        };
        Self {
            intermediate: measure_folder("Intermediate"),
            derived_data_cache: measure_folder("DerivedDataCache"),
            saved: measure_folder("Saved"),
            saved_config: measure_paths(&[project_dir.join("Saved").join("Config")]),
            binaries: measure_folder("Binaries"),
            plugins: measure_paths(&plugin_build_folders(project_dir)),
        }
    }

    fn displayed(&self, folder: CacheFolder, options: CleanupOptions) -> Option<u64> {
        match folder {
            CacheFolder::Intermediate => self.intermediate,
            CacheFolder::DerivedDataCache => self.derived_data_cache,
            CacheFolder::Saved => self.saved.map(|saved| {
                if options.keep_saved_config {
                    saved.saturating_sub(self.saved_config)
                } else {
                    saved
                }
            }),
            CacheFolder::Binaries => {
                let plugins = if options.include_plugin_folders {
                    self.plugins
                } else {
                    0
                };
                match self.binaries {
                    Some(binaries) => Some(binaries + plugins),
                    None if plugins > 0 => Some(plugins),
                    None => None,
                }
            }
        }
    }
}

pub struct CacheCleanupDialog {
    state: Entity<HubState>,
    project_key: String,
    project_name: String,
    project_dir: PathBuf,
    selected: Vec<CacheFolder>,
    options: CleanupOptions,
    mode: DeleteMode,
    sizes: Option<CacheSizes>,
    measure_task: Option<Task<()>>,
    clean_task: Option<Task<()>>,
}

impl EventEmitter<DialogEvent> for CacheCleanupDialog {}

impl CacheCleanupDialog {
    pub fn new(state: Entity<HubState>, project_key: String, cx: &mut Context<Self>) -> Self {
        let row = state.read(cx).row(&project_key).cloned();
        let project_name = row.as_ref().map(|row| row.name.clone()).unwrap_or_default();
        let project_dir = row
            .as_ref()
            .and_then(|row| row.path.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        let measure_dir = project_dir.clone();
        let measure = cx.background_spawn(async move { CacheSizes::measure(&measure_dir) });
        let measure_task = cx.spawn(async move |this, cx| {
            let sizes = measure.await;
            this.update(cx, |dialog, cx| {
                dialog.sizes = Some(sizes);
                dialog.measure_task = None;
                cx.notify();
            })
            .log_err();
        });
        Self {
            state,
            project_key,
            project_name,
            project_dir,
            selected: CacheFolder::ALL
                .into_iter()
                .filter(|folder| folder.selected_by_default())
                .collect(),
            options: CleanupOptions {
                keep_saved_config: true,
                include_plugin_folders: true,
            },
            mode: DeleteMode::Trash,
            sizes: None,
            measure_task: Some(measure_task),
            clean_task: None,
        }
    }

    fn is_available(&self, folder: CacheFolder) -> bool {
        self.sizes
            .is_some_and(|sizes| sizes.displayed(folder, self.options).is_some())
    }

    fn effective_selection(&self) -> Vec<CacheFolder> {
        self.selected
            .iter()
            .copied()
            .filter(|folder| self.is_available(*folder))
            .collect()
    }

    fn selected_total(&self) -> u64 {
        let Some(sizes) = self.sizes else {
            return 0;
        };
        self.effective_selection()
            .into_iter()
            .filter_map(|folder| sizes.displayed(folder, self.options))
            .sum()
    }

    fn toggle(&mut self, folder: CacheFolder, cx: &mut Context<Self>) {
        if let Some(position) = self
            .selected
            .iter()
            .position(|selected| *selected == folder)
        {
            self.selected.remove(position);
        } else {
            self.selected.push(folder);
        }
        cx.notify();
    }

    fn can_clean(&self, running: bool) -> bool {
        !running
            && self.sizes.is_some()
            && self.clean_task.is_none()
            && !self.effective_selection().is_empty()
    }

    pub fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.clean(cx);
    }

    fn clean(&mut self, cx: &mut Context<Self>) {
        let running = self.state.read(cx).is_project_running(&self.project_key);
        if !self.can_clean(running) {
            return;
        }
        let project_dir = self.project_dir.clone();
        let folders = self.effective_selection();
        let options = self.options;
        let mode = self.mode;
        let total = self.selected_total();
        let project_name = self.project_name.clone();
        let work = cx.background_spawn(async move {
            let targets: Vec<PathBuf> = folders
                .into_iter()
                .flat_map(|folder| cache_folder_targets(&project_dir, folder, options))
                .collect();
            delete_paths(&targets, mode)
        });
        let state = self.state.clone();
        self.clean_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<()> = work.await;
            let succeeded = result.is_ok();
            state
                .update(cx, |state, cx| match result {
                    Ok(()) => state.toast(
                        format!("{project_name} 캐시 {} 정리됨", format_bytes(total)),
                        ToastKind::Success,
                        cx,
                    ),
                    Err(error) => {
                        log::error!("{error:#}");
                        state.toast(format!("캐시 정리 실패: {error:#}"), ToastKind::Error, cx)
                    }
                })
                .log_err();
            this.update(cx, |dialog, cx| {
                dialog.clean_task = None;
                if succeeded {
                    cx.emit(DialogEvent::Close);
                }
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    fn folder_row(
        &self,
        folder: CacheFolder,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label: SharedString = match folder {
            CacheFolder::Intermediate => "Intermediate".into(),
            CacheFolder::DerivedDataCache => "DerivedDataCache".into(),
            CacheFolder::Saved => "Saved".into(),
            CacheFolder::Binaries => "Binaries".into(),
        };
        let note = match folder {
            CacheFolder::Saved => Some("로그·자동저장·세이브게임 포함"),
            CacheFolder::Binaries => Some("C++ 프로젝트는 재빌드 필요"),
            _ => None,
        };
        let size_label: AnyElement = match self.sizes {
            None => spinner_text(theme, "계산 중…").into_any_element(),
            Some(sizes) => match sizes.displayed(folder, self.options) {
                Some(bytes) => div().child(format_bytes(bytes)).into_any_element(),
                None => div()
                    .text_color(theme.text_disabled)
                    .child("없음")
                    .into_any_element(),
            },
        };
        let available = self.is_available(folder);
        let busy = self.clean_task.is_some();
        let checkbox = Checkbox::new(
            ("cache-folder", folder as usize),
            available && self.selected.contains(&folder),
            label,
        )
        .disabled(!available || busy)
        .on_click(cx.listener(move |this, _, _, cx| this.toggle(folder, cx)));
        let checkbox = match note {
            Some(note) => checkbox.detail(format!("({note})")),
            None => checkbox,
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(checkbox)
            .child(
                div()
                    .flex_none()
                    .text_color(theme.text_muted)
                    .child(size_label),
            )
    }
}

impl Render for CacheCleanupDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let running = self.state.read(cx).is_project_running(&self.project_key);
        let busy = self.clean_task.is_some();
        let can_clean = self.can_clean(running);
        let permanent = self.mode == DeleteMode::Permanent;
        let saved_selected =
            self.selected.contains(&CacheFolder::Saved) && self.is_available(CacheFolder::Saved);
        let binaries_selected = self.selected.contains(&CacheFolder::Binaries)
            && self.is_available(CacheFolder::Binaries);
        let clean_button = Button::new("cache-clean", if busy { "정리 중…" } else { "정리" })
            .disabled(!can_clean)
            .on_click(cx.listener(|this, _, _, cx| this.clean(cx)));
        let clean_button = if permanent {
            clean_button.danger()
        } else {
            clean_button.primary()
        };
        dialog_frame(
            &theme,
            format!("캐시 정리 — {}", self.project_name),
            34.0,
            cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
        )
        .child(
            dialog_body(&theme)
                .when(running, |this| {
                    this.child(error_note(
                        &theme,
                        "프로젝트가 실행 중이라 정리할 수 없습니다.",
                    ))
                })
                .child(self.folder_row(CacheFolder::Intermediate, &theme, cx))
                .child(self.folder_row(CacheFolder::DerivedDataCache, &theme, cx))
                .child(self.folder_row(CacheFolder::Saved, &theme, cx))
                .when(saved_selected, |this| {
                    this.child(
                        div().pl_6().child(
                            Checkbox::new(
                                "keep-saved-config",
                                self.options.keep_saved_config,
                                "Saved\\Config 보존",
                            )
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.options.keep_saved_config = !this.options.keep_saved_config;
                                cx.notify();
                            })),
                        ),
                    )
                })
                .child(self.folder_row(CacheFolder::Binaries, &theme, cx))
                .when(binaries_selected, |this| {
                    this.child(
                        div().pl_6().child(
                            Checkbox::new(
                                "include-plugin-folders",
                                self.options.include_plugin_folders,
                                "플러그인 Intermediate/Binaries도 포함",
                            )
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.options.include_plugin_folders =
                                    !this.options.include_plugin_folders;
                                cx.notify();
                            })),
                        ),
                    )
                })
                .child(div().h_px().bg(theme.border))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .font_weight(FontWeight::MEDIUM)
                        .child("선택 합계")
                        .child(if self.sizes.is_some() {
                            format_bytes(self.selected_total())
                        } else {
                            "계산 중…".to_string()
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_color(theme.text_muted).child("삭제 방식"))
                        .child(
                            Radio::new("delete-trash", !permanent)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.mode = DeleteMode::Trash;
                                    cx.notify();
                                }))
                                .child("휴지통으로"),
                        )
                        .child(
                            Radio::new("delete-permanent", permanent)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.mode = DeleteMode::Permanent;
                                    cx.notify();
                                }))
                                .child("영구 삭제"),
                        ),
                )
                .when(permanent, |this| {
                    this.child(error_note(&theme, "영구 삭제는 되돌릴 수 없습니다."))
                }),
        )
        .child(
            dialog_footer(&theme)
                .child(
                    Button::new("cache-cancel", "취소")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                )
                .child(clean_button),
        )
    }
}

pub struct RemoveEngineDialog {
    state: Entity<HubState>,
    engine: Option<Engine>,
    removal: EngineRemoval,
    confirm_input: Entity<TextInput>,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for RemoveEngineDialog {}

impl RemoveEngineDialog {
    pub fn new(state: Entity<HubState>, engine_key: String, cx: &mut Context<Self>) -> Self {
        let engine = state.read(cx).engine(&engine_key).cloned();
        let confirm_input = cx.new(|cx| TextInput::new("엔진 버전 입력", cx));
        let subscriptions =
            vec![cx.subscribe(&confirm_input, |_, _, _: &TextInputEvent, cx| cx.notify())];
        Self {
            state,
            engine,
            removal: EngineRemoval::UnregisterOnly,
            confirm_input,
            task: None,
            _subscriptions: subscriptions,
        }
    }

    fn can_remove(&self, cx: &Context<Self>) -> bool {
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if engine.source == EngineSource::Launcher
            || self.task.is_some()
            || self.state.read(cx).is_engine_running(&engine.key)
        {
            return false;
        }
        match self.removal {
            EngineRemoval::UnregisterOnly => true,
            EngineRemoval::DeleteFolder => {
                self.confirm_input.read(cx).text().trim() == engine.version_label()
            }
        }
    }

    pub fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let is_launcher = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.source == EngineSource::Launcher);
        if is_launcher {
            self.state
                .update(cx, |state, cx| state.open_epic_launcher(cx));
            cx.emit(DialogEvent::Close);
            return;
        }
        self.remove(cx);
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        if !self.can_remove(cx) {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let removal = self.removal;
        let task = self.state.update(cx, |state, cx| {
            state.remove_engine(&engine.key, removal, cx)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let succeeded = task.await.is_ok();
            this.update(cx, |dialog, cx| {
                dialog.task = None;
                if succeeded {
                    cx.emit(DialogEvent::Close);
                }
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }
}

impl Render for RemoveEngineDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let close = cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close));
        let Some(engine) = self.engine.clone() else {
            return dialog_frame(&theme, "엔진 제거", 30.0, close)
                .child(dialog_body(&theme).child("엔진을 찾을 수 없습니다."))
                .into_any_element();
        };
        let title = format!("엔진 제거 — {}", engine.label());
        if engine.source == EngineSource::Launcher {
            return dialog_frame(&theme, title, 30.0, close)
                .child(
                    dialog_body(&theme)
                        .child("Launcher로 설치된 엔진은 Epic Games Launcher에서 제거해야 합니다."),
                )
                .child(
                    dialog_footer(&theme)
                        .child(
                            Button::new("remove-close", "닫기")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                        )
                        .child(
                            Button::new("remove-open-launcher", "Epic Launcher 열기")
                                .primary()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.confirm(window, cx)),
                                ),
                        ),
                )
                .into_any_element();
        }
        let hub = self.state.read(cx);
        let projects_using = hub.projects_using_engine(&engine.key);
        let running = hub.is_engine_running(&engine.key);
        let busy = self.task.is_some();
        let can_remove = self.can_remove(cx);
        let delete_folder = self.removal == EngineRemoval::DeleteFolder;
        let unregister_label = match engine.source {
            EngineSource::SourceBuild => "목록·레지스트리 등록만 해제",
            _ => "목록에서만 제거",
        };
        let confirm_input = self.confirm_input.clone();
        let remove_button = Button::new(
            "remove-apply",
            if busy {
                "제거 중…"
            } else if delete_folder {
                "삭제"
            } else {
                "제거"
            },
        )
        .disabled(!can_remove)
        .on_click(cx.listener(|this, _, _, cx| this.remove(cx)));
        let remove_button = if delete_folder {
            remove_button.danger()
        } else {
            remove_button.primary()
        };
        dialog_frame(&theme, title, 32.0, close)
            .child(
                dialog_body(&theme)
                    .when(running, |this| {
                        this.child(error_note(&theme, "엔진이 실행 중이라 제거할 수 없습니다."))
                    })
                    .when(!projects_using.is_empty(), |this| {
                        this.child(warning_note(
                            &theme,
                            format!(
                                "이 엔진을 사용하는 프로젝트 {}개: {}",
                                projects_using.len(),
                                projects_using.join(", ")
                            ),
                        ))
                    })
                    .child(
                        Radio::new("removal-unregister", !delete_folder)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.removal = EngineRemoval::UnregisterOnly;
                                cx.notify();
                            }))
                            .child(unregister_label),
                    )
                    .child(
                        Radio::new("removal-delete", delete_folder)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.removal = EngineRemoval::DeleteFolder;
                                let focus = this.confirm_input.focus_handle(cx);
                                window.focus(&focus);
                                cx.notify();
                            }))
                            .child("폴더까지 삭제 (휴지통)"),
                    )
                    .when(delete_folder, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .pl_6()
                                .child(div().text_color(theme.text_muted).child(format!(
                                    "확인을 위해 엔진 버전 {}을(를) 입력하세요.",
                                    engine.version_label()
                                )))
                                .child(div().w(rems(14.0)).flex().child(confirm_input)),
                        )
                    }),
            )
            .child(
                dialog_footer(&theme)
                    .child(
                        Button::new("remove-cancel", "취소")
                            .disabled(busy)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(remove_button),
            )
            .into_any_element()
    }
}
