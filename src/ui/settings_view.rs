use crate::paths::display_path;
use crate::platform;
use crate::state::{HubState, ToastKind};
use crate::storage::{Settings, ThemePreference, app_data_dir};
use crate::ui::components::{Button, Icon, IconButton, Radio, Toggle, dialog_frame, section_label};
use crate::ui::dialogs::DialogEvent;
use crate::ui::theme::Theme;
use crate::util::ResultExt;
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, PathPromptOptions, Render,
    SharedString, Subscription, Task, Window, div, prelude::*, rems,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    General,
    Projects,
    Appearance,
    Integration,
    About,
}

impl Category {
    const ALL: [Category; 5] = [
        Category::General,
        Category::Projects,
        Category::Appearance,
        Category::Integration,
        Category::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Category::General => "일반",
            Category::Projects => "프로젝트",
            Category::Appearance => "외관",
            Category::Integration => "연동",
            Category::About => "정보",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FolderTarget {
    NewProjectDirectory,
    WatchFolder,
}

pub struct SettingsDialog {
    state: Entity<HubState>,
    category: Category,
    detected_launcher: Option<PathBuf>,
    detect_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for SettingsDialog {}

impl SettingsDialog {
    pub fn new(state: Entity<HubState>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        let mut dialog = Self {
            state,
            category: Category::General,
            detected_launcher: None,
            detect_task: None,
            _subscriptions: subscriptions,
        };
        dialog.detect_launcher(cx);
        dialog
    }

    fn detect_launcher(&mut self, cx: &mut Context<Self>) {
        let detect = cx.background_spawn(async move { platform::detect_epic_launcher() });
        self.detect_task = Some(cx.spawn(async move |this, cx| {
            let detected = detect.await;
            this.update(cx, |dialog, cx| {
                dialog.detected_launcher = detected;
                dialog.detect_task = None;
                cx.notify();
            })
            .log_err();
        }));
    }

    fn update_settings(&mut self, cx: &mut Context<Self>, update: impl FnOnce(&mut Settings)) {
        self.state
            .update(cx, |state, cx| state.update_settings(cx, update));
    }

    fn pick_folder(&mut self, target: FolderTarget, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("폴더 선택".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let folder = match receiver.await {
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
            let Some(folder) = folder else {
                return;
            };
            state
                .update(cx, |state, cx| {
                    state.update_settings(cx, |settings| match target {
                        FolderTarget::NewProjectDirectory => {
                            settings.new_project_directory = Some(folder)
                        }
                        FolderTarget::WatchFolder => {
                            if !settings.watch_folders.contains(&folder) {
                                settings.watch_folders.push(folder);
                            }
                        }
                    })
                })
                .log_err();
        })
        .detach();
    }

    fn pick_launcher(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("EpicGamesLauncher.exe 선택".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                state
                    .update(cx, |state, cx| {
                        state.update_settings(cx, |settings| {
                            settings.epic_launcher_path = Some(path)
                        })
                    })
                    .log_err();
            }
        })
        .detach();
    }

    fn setting_row(
        theme: &Theme,
        label: impl Into<SharedString>,
        description: Option<&'static str>,
        control: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .py_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_sm().child(label.into()))
                    .when_some(description, |this, description| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(description),
                        )
                    }),
            )
            .child(control)
    }

    fn render_general(
        &self,
        settings: &Settings,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .child(Self::setting_row(
                theme,
                "에디터 실행 후 Hub 최소화",
                None,
                Toggle::new("minimize-after-launch", settings.minimize_after_launch).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.update_settings(cx, |settings| {
                            settings.minimize_after_launch = !settings.minimize_after_launch
                        })
                    }),
                ),
            ))
            .into_any_element()
    }

    fn render_projects(
        &self,
        settings: &Settings,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hub = self.state.read(cx);
        let engines: Vec<(String, String)> = hub
            .installed_engines()
            .map(|engine| (engine.key.clone(), engine.label()))
            .collect();
        let automatic_default = settings.default_engine.is_none()
            || !engines
                .iter()
                .any(|(key, _)| Some(key) == settings.default_engine.as_ref());
        let new_project_directory = settings
            .new_project_directory_or_default()
            .map(|path| display_path(&path))
            .unwrap_or_else(|| "설정되지 않음".to_string());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::setting_row(
                theme,
                "새 프로젝트 기본 위치",
                None,
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .max_w(rems(16.0))
                            .truncate()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(new_project_directory),
                    )
                    .child(Button::new("pick-new-project-directory", "변경").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.pick_folder(FolderTarget::NewProjectDirectory, cx)
                        }),
                    )),
            ))
            .child(section_label(theme, "기본 엔진 버전"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        Radio::new("default-engine-auto", automatic_default)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_settings(cx, |settings| settings.default_engine = None)
                            }))
                            .child("가장 높은 버전 (자동)"),
                    )
                    .children(
                        engines
                            .into_iter()
                            .enumerate()
                            .map(|(index, (key, label))| {
                                let selected = !automatic_default
                                    && settings.default_engine.as_ref() == Some(&key);
                                Radio::new(("default-engine", index), selected)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let key = key.clone();
                                        this.update_settings(cx, |settings| {
                                            settings.default_engine = Some(key)
                                        })
                                    }))
                                    .child(label)
                            }),
                    ),
            )
            .child(div().mt_2().child(section_label(theme, "감시 폴더")))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("하위 3단계까지 .uproject를 찾고, 새로 생기면 자동으로 추가합니다."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .when(settings.watch_folders.is_empty(), |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("감시 폴더 없음"),
                        )
                    })
                    .children(
                        settings
                            .watch_folders
                            .iter()
                            .enumerate()
                            .map(|(index, folder)| {
                                let folder_to_remove = folder.clone();
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_3()
                                    .pr_1()
                                    .py_1()
                                    .when(index > 0, |this| {
                                        this.border_t_1().border_color(theme.border)
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .child(display_path(folder)),
                                    )
                                    .child(
                                        IconButton::new(
                                            ("remove-watch-folder", index),
                                            Icon::Close,
                                        )
                                        .size(0.8)
                                        .tooltip("제거")
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                let folder = folder_to_remove.clone();
                                                this.update_settings(cx, |settings| {
                                                    settings
                                                        .watch_folders
                                                        .retain(|existing| *existing != folder)
                                                })
                                            }),
                                        ),
                                    )
                            }),
                    ),
            )
            .child(
                div().flex().child(
                    Button::new("add-watch-folder", "폴더 추가")
                        .leading_icon(Icon::Plus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pick_folder(FolderTarget::WatchFolder, cx)
                        })),
                ),
            )
            .child(Self::setting_row(
                theme,
                "시작 시 엔진 최근 프로젝트 가져오기",
                Some("설치된 엔진의 EditorSettings.ini에서 최근 프로젝트를 읽습니다."),
                Toggle::new(
                    "import-engine-recent",
                    settings.import_engine_recent_on_startup,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.update_settings(cx, |settings| {
                        settings.import_engine_recent_on_startup =
                            !settings.import_engine_recent_on_startup
                    })
                })),
            ))
            .into_any_element()
    }

    fn render_appearance(
        &self,
        settings: &Settings,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme_option = |id: &'static str, label: &'static str, preference: ThemePreference| {
            Radio::new(id, settings.theme == preference)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_settings(cx, |settings| settings.theme = preference)
                }))
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_label(theme, "테마"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(theme_option("theme-dark", "다크", ThemePreference::Dark))
                    .child(theme_option(
                        "theme-light",
                        "라이트",
                        ThemePreference::Light,
                    ))
                    .child(theme_option(
                        "theme-system",
                        "시스템",
                        ThemePreference::System,
                    )),
            )
            .child(div().mt_2().child(section_label(theme, "UI 크기")))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children(Settings::UI_SCALES.into_iter().map(|scale| {
                        let selected = settings.ui_scale == scale;
                        div()
                            .id(("ui-scale", scale as usize))
                            .px_2p5()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(if selected {
                                theme.accent
                            } else {
                                theme.border_strong
                            })
                            .when(selected, |this| this.bg(theme.highlight))
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.hover))
                            .text_sm()
                            .child(format!("{scale}%"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_settings(cx, |settings| settings.ui_scale = scale)
                            }))
                    })),
            )
            .into_any_element()
    }

    fn render_integration(
        &self,
        settings: &Settings,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let configured = settings.epic_launcher_path.clone();
        let current = match (&configured, &self.detected_launcher) {
            (Some(path), _) => display_path(path),
            (None, Some(detected)) => format!("자동 감지됨: {}", display_path(detected)),
            (None, None) if self.detect_task.is_some() => "감지 중…".to_string(),
            (None, None) => "찾지 못함 (com.epicgames.launcher:// 링크로 실행)".to_string(),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_label(theme, "Epic Games Launcher 경로"))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.input)
                    .border_1()
                    .border_color(theme.border)
                    .text_sm()
                    .truncate()
                    .child(current),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("pick-launcher", "찾아보기")
                            .on_click(cx.listener(|this, _, _, cx| this.pick_launcher(cx))),
                    )
                    .child(
                        Button::new("detect-launcher", "자동 감지").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.update_settings(cx, |settings| {
                                    settings.epic_launcher_path = None
                                });
                                this.detect_launcher(cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_about(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Unreal Hub"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(format!("버전 {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(div().flex().mt_2().child(
                Button::new("open-log-folder", "로그 폴더 열기").on_click(cx.listener(
                    |this, _, _, cx| match app_data_dir() {
                        Ok(directory) => cx.open_with_system(&directory.join("logs")),
                        Err(error) => this.state.update(cx, |state, cx| {
                            state.toast(format!("{error:#}"), ToastKind::Error, cx)
                        }),
                    },
                )),
            ))
            .into_any_element()
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let settings = self.state.read(cx).settings.clone();
        let content = match self.category {
            Category::General => self.render_general(&settings, &theme, cx),
            Category::Projects => self.render_projects(&settings, &theme, cx),
            Category::Appearance => self.render_appearance(&settings, &theme, cx),
            Category::Integration => self.render_integration(&settings, &theme, cx),
            Category::About => self.render_about(&theme, cx),
        };
        dialog_frame(
            &theme,
            "설정",
            44.0,
            cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
        )
        .h(rems(32.0))
        .child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .gap_0p5()
                        .w(rems(9.0))
                        .p_2()
                        .border_r_1()
                        .border_color(theme.border)
                        .children(Category::ALL.into_iter().map(|category| {
                            let selected = self.category == category;
                            div()
                                .id(category.label())
                                .px_3()
                                .py_1p5()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .text_color(if selected {
                                    theme.text
                                } else {
                                    theme.text_muted
                                })
                                .when(selected, |this| this.bg(theme.selected))
                                .when(!selected, |this| this.hover(|style| style.bg(theme.hover)))
                                .child(category.label())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.category = category;
                                    cx.notify();
                                }))
                        })),
                )
                .child(
                    div()
                        .id("settings-content")
                        .flex_1()
                        .min_w_0()
                        .p_4()
                        .overflow_y_scroll()
                        .child(content),
                ),
        )
    }
}
