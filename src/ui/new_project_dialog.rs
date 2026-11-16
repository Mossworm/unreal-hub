use crate::engines::Engine;
use crate::paths::display_path;
use crate::platform;
use crate::project_creation::{
    Cancelled, CreatedProject, CreationRequest, CreationStage, GraphicsPreset, HardwareClass,
    create_project, suggest_project_name, validate_location, validate_project_name,
};
use crate::state::{HubState, ToastKind};
use crate::templates::{TemplateCatalog, TemplateGroup, TemplateSetting, load_catalog};
use crate::ui::components::{
    Button, Icon, IconButton, Toggle, dialog_body, dialog_footer, dialog_frame, error_note, icon,
    menu_container, menu_item, section_label, spinner_text, tooltip, warning_note,
};
use crate::ui::dialogs::DialogEvent;
use crate::ui::text_input::{TextInput, TextInputEvent};
use crate::ui::theme::Theme;
use crate::util::ResultExt;
use anyhow::{Result, anyhow};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{
    AnyElement, Context, Div, ElementId, Entity, EventEmitter, FontWeight, IntoElement, ObjectFit,
    PathPromptOptions, Render, SharedString, Stateful, Subscription, Task, Window, deferred, div,
    img, prelude::*, rems,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_PROJECT_NAME: &str = "MyProject";

struct Creation {
    stage: Option<CreationStage>,
    cancel: Arc<AtomicBool>,
    cancelling: bool,
    _task: Task<()>,
}

pub struct NewProjectDialog {
    state: Entity<HubState>,
    engines: Vec<Engine>,
    engine_key: Option<String>,
    engine_menu_open: bool,
    catalog: Option<TemplateCatalog>,
    catalog_error: Option<String>,
    catalog_task: Option<Task<()>>,
    selected_group: Option<String>,
    use_cpp: bool,
    hardware: HardwareClass,
    graphics: GraphicsPreset,
    name_input: Entity<TextInput>,
    name_edited: bool,
    location: Option<PathBuf>,
    validated: Option<(String, Option<PathBuf>, Option<String>)>,
    validation_task: Option<Task<()>>,
    open_after: bool,
    has_cpp_toolchain: Option<bool>,
    _toolchain_task: Task<()>,
    creation: Option<Creation>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for NewProjectDialog {}

impl Drop for NewProjectDialog {
    fn drop(&mut self) {
        if let Some(creation) = &self.creation {
            creation.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl NewProjectDialog {
    pub fn new(state: Entity<HubState>, cx: &mut Context<Self>) -> Self {
        let hub = state.read(cx);
        let engines: Vec<Engine> = hub.installed_engines().cloned().collect();
        let engine_key = hub.default_engine().map(|engine| engine.key.clone());
        let location = hub.settings.new_project_directory_or_default();
        let open_after = hub.settings.open_after_create;
        let name_input = cx.new(|cx| {
            let mut input = TextInput::new(DEFAULT_PROJECT_NAME, cx);
            input.set_text(DEFAULT_PROJECT_NAME, cx);
            input
        });
        let subscriptions = vec![cx.subscribe(
            &name_input,
            |this, _, event: &TextInputEvent, cx| match event {
                TextInputEvent::Changed => {
                    this.name_edited = true;
                    this.validate(cx);
                }
            },
        )];
        let toolchain = cx.background_spawn(async move { platform::has_cpp_toolchain() });
        let toolchain_task = cx.spawn(async move |this, cx| {
            let available = toolchain.await;
            this.update(cx, |dialog, cx| {
                dialog.has_cpp_toolchain = Some(available);
                cx.notify();
            })
            .log_err();
        });
        let mut dialog = Self {
            state,
            engines,
            engine_key,
            engine_menu_open: false,
            catalog: None,
            catalog_error: None,
            catalog_task: None,
            selected_group: None,
            use_cpp: false,
            hardware: HardwareClass::Desktop,
            graphics: GraphicsPreset::Maximum,
            name_input,
            name_edited: false,
            location,
            validated: None,
            validation_task: None,
            open_after,
            has_cpp_toolchain: None,
            _toolchain_task: toolchain_task,
            creation: None,
            error: None,
            _subscriptions: subscriptions,
        };
        dialog.load_templates(cx);
        dialog.suggest_name(cx);
        dialog
    }

    fn engine(&self) -> Option<&Engine> {
        self.engine_key
            .as_deref()
            .and_then(|key| self.engines.iter().find(|engine| engine.key == key))
    }

    fn group(&self) -> Option<&TemplateGroup> {
        let catalog = self.catalog.as_ref()?;
        self.selected_group
            .as_deref()
            .and_then(|key| catalog.group(key))
    }

    fn project_name(&self, cx: &Context<Self>) -> String {
        self.name_input.read(cx).text().trim().to_string()
    }

    fn load_templates(&mut self, cx: &mut Context<Self>) {
        self.catalog = None;
        self.catalog_error = None;
        let Some(engine_root) = self.engine().map(|engine| engine.root.clone()) else {
            return;
        };
        let load = cx.background_spawn(async move { load_catalog(&engine_root) });
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |dialog, cx| {
                dialog.catalog_task = None;
                match result {
                    Ok(catalog) => {
                        let keep = dialog
                            .selected_group
                            .as_deref()
                            .and_then(|key| catalog.group(key))
                            .map(|group| group.key.clone());
                        dialog.selected_group =
                            keep.or_else(|| catalog.default_group().map(|group| group.key.clone()));
                        dialog.catalog = Some(catalog);
                        dialog.normalize_project_type();
                    }
                    Err(error) => dialog.catalog_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    fn normalize_project_type(&mut self) {
        let (has_blueprint, has_cpp) = self
            .group()
            .map(|group| (group.blueprint.is_some(), group.cpp.is_some()))
            .unwrap_or((true, false));
        if self.use_cpp && !has_cpp {
            self.use_cpp = false;
        }
        if !self.use_cpp && !has_blueprint && has_cpp {
            self.use_cpp = true;
        }
    }

    fn suggest_name(&mut self, cx: &mut Context<Self>) {
        let Some(location) = self.location.clone() else {
            self.validate(cx);
            return;
        };
        let suggest = cx
            .background_spawn(async move { suggest_project_name(&location, DEFAULT_PROJECT_NAME) });
        self.validation_task = Some(cx.spawn(async move |this, cx| {
            let suggestion = suggest.await;
            this.update(cx, |dialog, cx| {
                if !dialog.name_edited {
                    dialog
                        .name_input
                        .update(cx, |input, cx| input.set_text(suggestion, cx));
                    dialog.name_edited = false;
                }
                dialog.validate(cx);
            })
            .log_err();
        }));
    }

    fn validate(&mut self, cx: &mut Context<Self>) {
        let name = self.project_name(cx);
        let location = self.location.clone();
        let engine_root = self.engine().map(|engine| engine.root.clone());
        if validate_project_name(&name).is_err() {
            self.validated = Some((name, location, None));
            cx.notify();
            return;
        }
        let check = cx.background_spawn({
            let name = name.clone();
            let location = location.clone();
            async move {
                match (location, engine_root) {
                    (None, _) => Some("위치를 선택하세요.".to_string()),
                    (Some(location), Some(engine_root)) => {
                        validate_location(&location, &name, &engine_root).err()
                    }
                    (Some(location), None) => {
                        validate_location(&location, &name, &PathBuf::new()).err()
                    }
                }
            }
        });
        self.validation_task = Some(cx.spawn(async move |this, cx| {
            let error = check.await;
            this.update(cx, |dialog, cx| {
                dialog.validated = Some((name, location, error));
                cx.notify();
            })
            .log_err();
        }));
    }

    fn name_error(&self, cx: &Context<Self>) -> Option<String> {
        validate_project_name(&self.project_name(cx)).err()
    }

    fn location_error(&self, cx: &Context<Self>) -> Option<String> {
        let name = self.project_name(cx);
        match &self.validated {
            Some((validated_name, validated_location, error))
                if *validated_name == name && *validated_location == self.location =>
            {
                error.clone()
            }
            _ => None,
        }
    }

    fn is_validated(&self, cx: &Context<Self>) -> bool {
        let name = self.project_name(cx);
        self.validated
            .as_ref()
            .is_some_and(|(validated_name, validated_location, _)| {
                *validated_name == name && *validated_location == self.location
            })
    }

    fn can_create(&self, cx: &Context<Self>) -> bool {
        self.creation.is_none()
            && self.engine().is_some()
            && self.group().is_some()
            && self.name_error(cx).is_none()
            && self.is_validated(cx)
            && self.location_error(cx).is_none()
    }

    fn select_engine(&mut self, key: String, cx: &mut Context<Self>) {
        self.engine_menu_open = false;
        if self.engine_key.as_deref() == Some(key.as_str()) {
            cx.notify();
            return;
        }
        self.engine_key = Some(key);
        self.load_templates(cx);
        self.validate(cx);
    }

    fn select_group(&mut self, key: String, cx: &mut Context<Self>) {
        self.selected_group = Some(key);
        self.normalize_project_type();
        cx.notify();
    }

    fn pick_location(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("프로젝트 위치 선택".into()),
        });
        cx.spawn(async move |this, cx| {
            let folder = match receiver.await {
                Ok(Ok(paths)) => paths.and_then(|paths| paths.into_iter().next()),
                Ok(Err(error)) => {
                    log::error!("Folder picker failed: {error:#}");
                    None
                }
                Err(_) => None,
            };
            if let Some(folder) = folder {
                this.update(cx, |dialog, cx| {
                    dialog.location = Some(folder);
                    if dialog.name_edited {
                        dialog.validate(cx);
                    } else {
                        dialog.suggest_name(cx);
                    }
                })
                .log_err();
            }
        })
        .detach();
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        match &mut self.creation {
            Some(creation) => {
                creation.cancel.store(true, Ordering::Relaxed);
                creation.cancelling = true;
                cx.notify();
                false
            }
            None => true,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.request_close(cx) {
            cx.emit(DialogEvent::Close);
        }
    }

    pub fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.create(cx);
    }

    fn build_request(&self, cx: &Context<Self>) -> Result<CreationRequest> {
        let engine = self.engine().ok_or_else(|| anyhow!("엔진을 선택하세요"))?;
        let group = self.group().ok_or_else(|| anyhow!("템플릿을 선택하세요"))?;
        let template = if self.use_cpp {
            group.cpp.clone()
        } else {
            group.blueprint.clone()
        }
        .ok_or_else(|| anyhow!("선택한 프로젝트 유형을 지원하지 않는 템플릿입니다"))?;
        let location = self
            .location
            .clone()
            .ok_or_else(|| anyhow!("위치를 선택하세요"))?;
        Ok(CreationRequest {
            engine_root: engine.root.clone(),
            engine_identifier: engine.identifier.clone(),
            engine_version: engine.version,
            template,
            project_name: self.project_name(cx),
            location,
            hardware: self.hardware,
            graphics: self.graphics,
        })
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if !self.can_create(cx) {
            return;
        }
        let request = match self.build_request(cx) {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        self.error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let (progress_sender, mut progress_receiver) = mpsc::unbounded::<CreationStage>();
        let (result_sender, result_receiver) = oneshot::channel::<Result<CreatedProject>>();
        let worker_cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("project-creation".into())
            .spawn(move || {
                let result = create_project(&request, &worker_cancel, &mut |stage| {
                    if progress_sender.unbounded_send(stage).is_err() {
                        log::debug!("Creation progress receiver closed");
                    }
                });
                drop(progress_sender);
                if result_sender.send(result).is_err() {
                    log::warn!("Project creation finished after the dialog closed");
                }
            });
        if let Err(error) = spawned {
            self.error = Some(format!("작업 스레드를 시작하지 못했습니다: {error}"));
            cx.notify();
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            while let Some(stage) = progress_receiver.next().await {
                let updated = this.update(cx, |dialog, cx| {
                    if let Some(creation) = &mut dialog.creation {
                        creation.stage = Some(stage);
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
            let result = result_receiver
                .await
                .unwrap_or_else(|_| Err(anyhow!("프로젝트 생성 작업이 중단되었습니다")));
            this.update(cx, |dialog, cx| dialog.finish(result, cx))
                .log_err();
        });
        self.creation = Some(Creation {
            stage: None,
            cancel,
            cancelling: false,
            _task: task,
        });
        cx.notify();
    }

    fn finish(&mut self, result: Result<CreatedProject>, cx: &mut Context<Self>) {
        self.creation = None;
        match result {
            Ok(created) => {
                let open_after = self.open_after;
                let name = self.project_name(cx);
                let location = self.location.clone();
                self.state.update(cx, |state, cx| {
                    state.update_settings(cx, |settings| {
                        settings.open_after_create = open_after;
                        if location.is_some() && settings.new_project_directory.is_none() {
                            settings.new_project_directory = location;
                        }
                    });
                    match &created.project_files_error {
                        None => state.toast(
                            format!("프로젝트 '{name}'을(를) 만들었습니다"),
                            ToastKind::Success,
                            cx,
                        ),
                        Some(error) => state.toast(
                            format!(
                                "프로젝트를 만들었지만 프로젝트 파일 생성에 실패했습니다: {error}"
                            ),
                            ToastKind::Error,
                            cx,
                        ),
                    }
                    state.register_created_project(created.project_file.clone(), open_after, cx);
                });
                cx.emit(DialogEvent::Close);
            }
            Err(error) if error.is::<Cancelled>() => {
                self.state.update(cx, |state, cx| {
                    state.toast("프로젝트 생성을 취소했습니다", ToastKind::Info, cx)
                });
                cx.emit(DialogEvent::Close);
            }
            Err(error) => {
                log::error!("Project creation failed: {error:#}");
                self.error = Some(format!("{error:#}"));
                self.validate(cx);
            }
        }
        cx.notify();
    }

    fn render_no_engine(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        dialog_frame(
            theme,
            "새 프로젝트",
            30.0,
            cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
        )
        .child(dialog_body(theme).child("설치된 엔진이 없습니다. 설치 탭에서 엔진을 추가하세요."))
        .child(
            dialog_footer(theme)
                .child(
                    Button::new("no-engine-close", "닫기")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                )
                .child(
                    Button::new("no-engine-installs", "설치 탭으로 이동")
                        .primary()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::ShowInstalls))),
                ),
        )
        .into_any_element()
    }

    fn render_engine_picker(
        &self,
        theme: &Theme,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = self
            .engine()
            .map(|engine| engine.label())
            .unwrap_or_else(|| "엔진 선택".to_string());
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_color(theme.text_muted).child("엔진 버전"))
            .child(
                div()
                    .relative()
                    .child(
                        Button::new("engine-picker", label)
                            .trailing_icon(Icon::ChevronDown)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.engine_menu_open = !this.engine_menu_open;
                                cx.notify();
                            })),
                    )
                    .when(self.engine_menu_open, |this| {
                        this.child(
                            div().absolute().top(rems(2.25)).left_0().child(
                                deferred(
                                    menu_container(theme)
                                        .id("engine-picker-menu")
                                        .occlude()
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.engine_menu_open = false;
                                            cx.notify();
                                        }))
                                        .children(self.engines.iter().enumerate().map(
                                            |(index, engine)| {
                                                let key = engine.key.clone();
                                                menu_item(
                                                    ("engine-option", index),
                                                    engine.label(),
                                                    false,
                                                    None,
                                                    theme,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_engine(key.clone(), cx)
                                                }))
                                            },
                                        )),
                                )
                                .with_priority(1),
                            ),
                        )
                    }),
            )
    }

    fn render_template_tile(
        &self,
        group: &TemplateGroup,
        id: ElementId,
        theme: &Theme,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected_group.as_deref() == Some(group.key.as_str());
        let key = group.key.clone();
        let thumbnail: AnyElement = match group.thumbnail() {
            Some(path) => img(path.clone())
                .size(rems(5.0))
                .rounded_md()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .justify_center()
                .size(rems(5.0))
                .rounded_md()
                .bg(theme.elevated)
                .child(icon(Icon::Cube, 2.0, theme.text_muted))
                .into_any_element(),
        };
        div()
            .id(id)
            .flex()
            .flex_col()
            .items_center()
            .gap_1p5()
            .w(rems(6.5))
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(if selected {
                theme.accent
            } else {
                gpui::transparent_black()
            })
            .when(selected, |this| this.bg(theme.highlight))
            .when(!busy, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(theme.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_group(key.clone(), cx)))
            })
            .child(thumbnail)
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_center()
                    .line_clamp(2)
                    .child(group.display_name.clone()),
            )
    }

    fn render_templates(&self, theme: &Theme, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        if let Some(error) = &self.catalog_error {
            return div()
                .p_4()
                .child(error_note(theme, error.clone()))
                .into_any_element();
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return div()
                .p_4()
                .child(spinner_text(theme, "템플릿을 읽는 중…"))
                .into_any_element();
        };
        if catalog.groups.is_empty() {
            return div()
                .p_4()
                .child(error_note(
                    theme,
                    "이 엔진에서 사용할 수 있는 템플릿이 없습니다.",
                ))
                .into_any_element();
        }
        let mut sections = Vec::new();
        for (category_index, category) in catalog.categories.iter().enumerate() {
            let mut tiles = Vec::new();
            for (index, group) in catalog.groups_in(&category.key).into_iter().enumerate() {
                let id = ElementId::NamedInteger(
                    "template-tile".into(),
                    (category_index * 1000 + index) as u64,
                );
                tiles.push(
                    self.render_template_tile(group, id, theme, busy, cx)
                        .into_any_element(),
                );
            }
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(section_label(theme, category.display_name.clone()))
                    .child(div().flex().flex_wrap().gap_1().children(tiles)),
            );
        }
        div()
            .id("template-grid")
            .size_full()
            .overflow_y_scroll()
            .child(div().flex().flex_col().gap_3().p_3().children(sections))
            .into_any_element()
    }

    fn segment(
        id: impl Into<ElementId>,
        label: &'static str,
        selected: bool,
        disabled: bool,
        theme: &Theme,
    ) -> Stateful<Div> {
        let hover = theme.hover;
        div()
            .id(id)
            .px_3()
            .py_1()
            .text_sm()
            .border_1()
            .border_color(if selected {
                theme.accent
            } else {
                theme.border_strong
            })
            .when(selected, |this| this.bg(theme.highlight))
            .text_color(if disabled {
                theme.text_disabled
            } else {
                theme.text
            })
            .when(!disabled && !selected, |this| {
                this.cursor_pointer().hover(move |style| style.bg(hover))
            })
            .child(label)
    }

    fn option_row(
        theme: &Theme,
        label: &'static str,
        control: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(div().text_sm().text_color(theme.text_muted).child(label))
            .child(control)
    }

    fn render_details(&self, theme: &Theme, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(group) = self.group().cloned() else {
            return div().into_any_element();
        };
        let hides_hardware = group
            .representative()
            .is_some_and(|template| template.defs.hides(TemplateSetting::HardwareTarget));
        let hides_graphics = group
            .representative()
            .is_some_and(|template| template.defs.hides(TemplateSetting::GraphicsPreset));
        let preview: AnyElement = match group.preview() {
            Some(path) => img(path.clone())
                .w_full()
                .h(rems(9.0))
                .rounded_md()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(rems(9.0))
                .rounded_md()
                .bg(theme.elevated)
                .child(icon(Icon::Cube, 3.0, theme.text_disabled))
                .into_any_element(),
        };
        let name_error = self.name_error(cx);
        let location_error = self.location_error(cx);
        let location_label = self
            .location
            .as_ref()
            .map(|path| display_path(path))
            .unwrap_or_else(|| "위치를 선택하세요".into());
        let hidden_reason: SharedString = "이 템플릿에서는 설정할 수 없습니다".into();
        let hardware_disabled = busy || hides_hardware;
        let graphics_disabled = busy || hides_graphics;
        let show_toolchain_warning = self.use_cpp && self.has_cpp_toolchain == Some(false);
        let content = div()
            .flex()
            .flex_col()
            .gap_2p5()
            .p_3()
            .child(preview)
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(group.display_name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .line_clamp(5)
                    .child(group.description.clone()),
            )
            .child(Self::option_row(
                theme,
                "프로젝트 유형",
                div()
                    .flex()
                    .child(
                        Self::segment(
                            "type-blueprint",
                            "블루프린트",
                            !self.use_cpp,
                            busy || group.blueprint.is_none(),
                            theme,
                        )
                        .rounded_l_md()
                        .when(!busy && group.blueprint.is_some(), |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.use_cpp = false;
                                cx.notify();
                            }))
                        }),
                    )
                    .child(
                        Self::segment(
                            "type-cpp",
                            "C++",
                            self.use_cpp,
                            busy || group.cpp.is_none(),
                            theme,
                        )
                        .rounded_r_md()
                        .when(group.cpp.is_none(), |this| {
                            this.tooltip(tooltip("이 템플릿은 C++ 버전이 없습니다"))
                        })
                        .when(!busy && group.cpp.is_some(), |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.use_cpp = true;
                                cx.notify();
                            }))
                        }),
                    ),
            ))
            .child(Self::option_row(
                theme,
                "타깃 플랫폼",
                div()
                    .id("hardware-options")
                    .flex()
                    .when(hides_hardware, |this| {
                        this.tooltip(tooltip(hidden_reason.clone()))
                    })
                    .child(
                        Self::segment(
                            "hardware-desktop",
                            "데스크톱",
                            self.hardware == HardwareClass::Desktop,
                            hardware_disabled,
                            theme,
                        )
                        .rounded_l_md()
                        .when(!hardware_disabled, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.hardware = HardwareClass::Desktop;
                                cx.notify();
                            }))
                        }),
                    )
                    .child(
                        Self::segment(
                            "hardware-mobile",
                            "모바일",
                            self.hardware == HardwareClass::Mobile,
                            hardware_disabled,
                            theme,
                        )
                        .rounded_r_md()
                        .when(!hardware_disabled, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.hardware = HardwareClass::Mobile;
                                cx.notify();
                            }))
                        }),
                    ),
            ))
            .child(Self::option_row(
                theme,
                "품질 프리셋",
                div()
                    .id("graphics-options")
                    .flex()
                    .when(hides_graphics, |this| {
                        this.tooltip(tooltip(hidden_reason.clone()))
                    })
                    .child(
                        Self::segment(
                            "graphics-maximum",
                            "최대",
                            self.graphics == GraphicsPreset::Maximum,
                            graphics_disabled,
                            theme,
                        )
                        .rounded_l_md()
                        .when(!graphics_disabled, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.graphics = GraphicsPreset::Maximum;
                                cx.notify();
                            }))
                        }),
                    )
                    .child(
                        Self::segment(
                            "graphics-scalable",
                            "확장 가능",
                            self.graphics == GraphicsPreset::Scalable,
                            graphics_disabled,
                            theme,
                        )
                        .rounded_r_md()
                        .when(!graphics_disabled, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.graphics = GraphicsPreset::Scalable;
                                cx.notify();
                            }))
                        }),
                    ),
            ))
            .when(show_toolchain_warning, |this| {
                this.child(warning_note(
                    theme,
                    "C++ 프로젝트를 빌드하려면 Visual Studio가 필요합니다.",
                ))
            })
            .child(div().h_px().my_1().bg(theme.border))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child("프로젝트 이름"),
                    )
                    .child(div().flex().child(self.name_input.clone()))
                    .when_some(name_error, |this, error| {
                        this.child(div().text_xs().text_color(theme.danger).child(error))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().text_color(theme.text_muted).child("위치"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px_2()
                                    .py_1p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme.border_strong)
                                    .bg(theme.input)
                                    .text_sm()
                                    .truncate()
                                    .child(location_label),
                            )
                            .child(
                                IconButton::new("pick-project-location", Icon::FolderOpen)
                                    .tooltip("폴더 선택")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_location(cx))),
                            ),
                    )
                    .when_some(location_error, |this, error| {
                        this.child(div().text_xs().text_color(theme.danger).child(error))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("생성 후 바로 열기"))
                    .child(Toggle::new("open-after-create", self.open_after).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.open_after = !this.open_after;
                            cx.notify();
                        }),
                    )),
            );
        div()
            .id("template-details")
            .size_full()
            .overflow_y_scroll()
            .child(content)
            .into_any_element()
    }
}

impl Render for NewProjectDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        if self.engines.is_empty() {
            return self.render_no_engine(&theme, cx);
        }
        let busy = self.creation.is_some();
        let can_create = self.can_create(cx);
        let status: Option<AnyElement> = match &self.creation {
            Some(creation) if creation.cancelling => Some(
                spinner_text(&theme, "취소하는 중… (만들던 폴더를 정리합니다)").into_any_element(),
            ),
            Some(creation) => Some(
                spinner_text(
                    &theme,
                    creation
                        .stage
                        .map(CreationStage::label)
                        .unwrap_or("준비 중…"),
                )
                .into_any_element(),
            ),
            None => self.error.clone().map(|error| {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(error_note(&theme, error))
                    .into_any_element()
            }),
        };
        dialog_frame(
            &theme,
            "새 프로젝트",
            60.0,
            cx.listener(|this, _, _, cx| this.close(cx)),
        )
        .h(rems(40.0))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .child(self.render_engine_picker(&theme, busy, cx)),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_r_1()
                        .border_color(theme.border)
                        .child(self.render_templates(&theme, busy, cx)),
                )
                .child(
                    div()
                        .w(rems(22.0))
                        .flex_none()
                        .h_full()
                        .child(self.render_details(&theme, busy, cx)),
                ),
        )
        .child(
            dialog_footer(&theme)
                .children(status)
                .child(div().flex_1())
                .child(
                    Button::new(
                        "new-project-cancel",
                        if busy { "생성 취소" } else { "취소" },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                )
                .child(
                    Button::new("new-project-create", "프로젝트 생성")
                        .primary()
                        .disabled(!can_create)
                        .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                ),
        )
        .into_any_element()
    }
}
