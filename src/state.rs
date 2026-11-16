use crate::engines::{self, Engine, EngineSource, ManualEngine};
use crate::paths::{clean_path, path_key};
use crate::platform;
use crate::projects::{
    self, EPIC_CREATED_ROOT_DEPTH, ProjectInspection, ProjectOrigin, ProjectPresence,
    ProjectRecord, ProjectRow, WATCH_FOLDER_DEPTH,
};
use crate::storage::{
    CURRENT_SCHEMA_VERSION, ENGINES_FILE, EngineSizeRecord, EnginesFile, JsonFile, LoadedState,
    PROJECTS_FILE, PendingWrite, ProjectsFile, SETTINGS_FILE, Settings,
};
use crate::unreal::{is_uproject, new_engine_identifier};
use crate::util::{ResultExt, unix_now};
use anyhow::{Context as _, Result, anyhow, bail};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{AppContext as _, AsyncApp, Context, EventEmitter, SharedString, Task, WeakEntity};
use notify::{EventKind, RecursiveMode, Watcher as _};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub message: SharedString,
    pub kind: ToastKind,
}

pub enum HubEvent {
    Toast(Toast),
    RevealProject(String),
    EditorLaunched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineSizeState {
    Calculating,
    Known(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    Launching,
    AlreadyRunning,
    EngineMissing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineRemoval {
    UnregisterOnly,
    DeleteFolder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshScope {
    pub detect_engines: bool,
    pub import_recent: bool,
}

impl RefreshScope {
    fn merge(self, other: RefreshScope) -> RefreshScope {
        RefreshScope {
            detect_engines: self.detect_engines || other.detect_engines,
            import_recent: self.import_recent || other.import_recent,
        }
    }
}

struct RunningProcess {
    project_key: Option<String>,
    engine_key: Option<String>,
}

struct DiscoveredProject {
    path: PathBuf,
    origin: ProjectOrigin,
    last_open_time: Option<i64>,
}

struct RefreshResult {
    engines: Option<Vec<Engine>>,
    discovered: Vec<DiscoveredProject>,
    inspections: Vec<ProjectInspection>,
    imported_recent: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WatchPlan {
    recursive_roots: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
    launcher_directory: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchSignal {
    Projects,
    Engines,
}

pub struct HubState {
    pub settings: Settings,
    pub engines: Vec<Engine>,
    pub rows: Vec<ProjectRow>,
    pub engines_refreshing: bool,
    pub projects_refreshing: bool,
    manual_engines: Vec<ManualEngine>,
    engine_sizes: HashMap<String, EngineSizeState>,
    size_records: Vec<EngineSizeRecord>,
    records: Vec<ProjectRecord>,
    record_index: HashMap<String, usize>,
    running: HashMap<u32, RunningProcess>,
    process_waits: HashMap<u32, Task<()>>,
    settings_file: JsonFile,
    projects_file: JsonFile,
    engines_file: JsonFile,
    refresh_task: Option<Task<()>>,
    queued_refresh: Option<RefreshScope>,
    running_restored: bool,
    restore_running_task: Option<Task<()>>,
    size_task: Option<Task<()>>,
    watch_plan: WatchPlan,
    watcher: Option<notify::RecommendedWatcher>,
    watcher_task: Option<Task<()>>,
    watch_events_task: Option<Task<()>>,
    startup_toasts: Vec<Toast>,
}

impl EventEmitter<HubEvent> for HubState {}

impl HubState {
    pub fn new(loaded: LoadedState, cx: &mut Context<Self>) -> Self {
        let LoadedState {
            data_dir,
            settings,
            projects,
            engines,
        } = loaded;
        let mut startup_toasts = Vec::new();
        for (label, recovered, error) in [
            (
                "설정",
                settings.recovered_from_backup,
                settings.error.as_ref(),
            ),
            (
                "프로젝트 목록",
                projects.recovered_from_backup,
                projects.error.as_ref(),
            ),
            (
                "엔진 목록",
                engines.recovered_from_backup,
                engines.error.as_ref(),
            ),
        ] {
            if recovered {
                startup_toasts.push(Toast {
                    message: format!("{label} 파일이 손상되어 백업에서 복구했습니다").into(),
                    kind: ToastKind::Info,
                });
            } else if error.is_some() {
                startup_toasts.push(Toast {
                    message: format!("{label} 파일을 읽지 못해 초기화했습니다").into(),
                    kind: ToastKind::Error,
                });
            }
        }

        let mut seen = HashSet::new();
        let records: Vec<ProjectRecord> = projects
            .value
            .projects
            .into_iter()
            .filter(|record| seen.insert(record.key()))
            .collect();
        let engine_sizes = engines
            .value
            .sizes
            .iter()
            .map(|record| (record.key.clone(), EngineSizeState::Known(record.bytes)))
            .collect();
        let settings_value = settings.value;
        let import_recent = settings_value.import_engine_recent_on_startup
            || !settings_value.engine_recent_imported;

        let mut state = Self {
            settings: settings_value,
            engines: engines.value.detected,
            rows: Vec::new(),
            engines_refreshing: false,
            projects_refreshing: false,
            manual_engines: engines.value.manual,
            engine_sizes,
            size_records: engines.value.sizes,
            records,
            record_index: HashMap::new(),
            running: HashMap::new(),
            process_waits: HashMap::new(),
            settings_file: JsonFile::new(data_dir.join(SETTINGS_FILE)),
            projects_file: JsonFile::new(data_dir.join(PROJECTS_FILE)),
            engines_file: JsonFile::new(data_dir.join(ENGINES_FILE)),
            refresh_task: None,
            queued_refresh: None,
            running_restored: false,
            restore_running_task: None,
            size_task: None,
            watch_plan: WatchPlan::default(),
            watcher: None,
            watcher_task: None,
            watch_events_task: None,
            startup_toasts,
        };
        state.rebuild_rows();
        state.refresh(
            RefreshScope {
                detect_engines: true,
                import_recent,
            },
            cx,
        );
        state
    }

    pub fn take_startup_toasts(&mut self) -> Vec<Toast> {
        std::mem::take(&mut self.startup_toasts)
    }

    pub fn toast(
        &mut self,
        message: impl Into<SharedString>,
        kind: ToastKind,
        cx: &mut Context<Self>,
    ) {
        cx.emit(HubEvent::Toast(Toast {
            message: message.into(),
            kind,
        }));
    }

    pub fn engine(&self, key: &str) -> Option<&Engine> {
        self.engines.iter().find(|engine| engine.key == key)
    }

    pub fn row(&self, key: &str) -> Option<&ProjectRow> {
        self.rows.iter().find(|row| row.key == key)
    }

    pub fn installed_engines(&self) -> impl Iterator<Item = &Engine> {
        self.engines.iter().filter(|engine| engine.available)
    }

    pub fn engine_size(&self, key: &str) -> Option<EngineSizeState> {
        self.engine_sizes.get(key).copied()
    }

    pub fn is_project_running(&self, key: &str) -> bool {
        self.running_pid_for_project(key).is_some()
    }

    fn running_pid_for_project(&self, key: &str) -> Option<u32> {
        self.running
            .iter()
            .find(|(_, process)| process.project_key.as_deref() == Some(key))
            .map(|(pid, _)| *pid)
    }

    pub fn is_engine_running(&self, engine_key: &str) -> bool {
        self.running
            .values()
            .any(|process| process.engine_key.as_deref() == Some(engine_key))
    }

    pub fn projects_using_engine(&self, engine_key: &str) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| {
                row.resolution
                    .as_ref()
                    .and_then(|resolution| resolution.engine_key())
                    == Some(engine_key)
            })
            .map(|row| row.name.clone())
            .collect()
    }

    pub fn default_engine(&self) -> Option<&Engine> {
        self.settings
            .default_engine
            .as_deref()
            .and_then(|key| self.engine(key))
            .filter(|engine| engine.available)
            .or_else(|| self.installed_engines().next())
    }

    fn rebuild_rows(&mut self) {
        self.record_index = self
            .records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.key(), index))
            .collect();
        self.rows = self
            .records
            .iter()
            .filter(|record| record.presence != ProjectPresence::Gone)
            .map(|record| ProjectRow::from_record(record, &self.engines))
            .collect();
    }

    fn record_mut(&mut self, key: &str) -> Option<&mut ProjectRecord> {
        let index = *self.record_index.get(key)?;
        self.records.get_mut(index)
    }

    fn spawn_write(
        &self,
        pending: Result<PendingWrite>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        let pending = match pending {
            Ok(pending) => pending,
            Err(error) => {
                log::error!("Cannot serialize {label}: {error:#}");
                return;
            }
        };
        let write = cx.background_spawn(async move { pending.write() });
        cx.spawn(async move |this, cx| {
            if let Err(error) = write.await {
                log::error!("Failed to save {label}: {error:#}");
                this.update(cx, |state, cx| {
                    state.toast(
                        format!("{label}을(를) 저장하지 못했습니다"),
                        ToastKind::Error,
                        cx,
                    )
                })
                .log_err();
            }
        })
        .detach();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.version = CURRENT_SCHEMA_VERSION;
        let pending = self.settings_file.prepare(&self.settings);
        self.spawn_write(pending, "설정", cx);
    }

    fn save_projects(&mut self, cx: &mut Context<Self>) {
        let file = ProjectsFile {
            version: CURRENT_SCHEMA_VERSION,
            projects: self.records.clone(),
        };
        let pending = self.projects_file.prepare(&file);
        self.spawn_write(pending, "프로젝트 목록", cx);
    }

    fn save_engines(&mut self, cx: &mut Context<Self>) {
        let file = EnginesFile {
            version: CURRENT_SCHEMA_VERSION,
            manual: self.manual_engines.clone(),
            detected: self.engines.clone(),
            sizes: self.size_records.clone(),
        };
        let pending = self.engines_file.prepare(&file);
        self.spawn_write(pending, "엔진 목록", cx);
    }

    pub fn update_settings(&mut self, cx: &mut Context<Self>, update: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        update(&mut self.settings);
        if before == self.settings {
            return;
        }
        self.save_settings(cx);
        let watch_changed = before.watch_folders != self.settings.watch_folders;
        let import_enabled = !before.import_engine_recent_on_startup
            && self.settings.import_engine_recent_on_startup;
        if watch_changed || import_enabled {
            self.refresh(
                RefreshScope {
                    detect_engines: false,
                    import_recent: import_enabled,
                },
                cx,
            );
        }
        cx.notify();
    }

    pub fn refresh(&mut self, scope: RefreshScope, cx: &mut Context<Self>) {
        if self.refresh_task.is_some() {
            self.queued_refresh = Some(
                self.queued_refresh
                    .map_or(scope, |queued| queued.merge(scope)),
            );
            return;
        }
        let manual_engines = self.manual_engines.clone();
        let record_paths: Vec<PathBuf> = self
            .records
            .iter()
            .map(|record| record.path.clone())
            .collect();
        let watch_folders = self.settings.watch_folders.clone();
        self.engines_refreshing |= scope.detect_engines;
        self.projects_refreshing = true;
        cx.notify();
        let work = cx.background_spawn(async move {
            gather_refresh(scope, &manual_engines, &record_paths, &watch_folders)
        });
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |state, cx| state.apply_refresh(result, cx))
                .log_err();
        }));
    }

    fn apply_refresh(&mut self, result: RefreshResult, cx: &mut Context<Self>) {
        self.refresh_task = None;
        let engines_changed = match result.engines {
            Some(engines) => {
                self.engines_refreshing = false;
                let changed = engines != self.engines;
                self.engines = engines;
                changed
            }
            None => false,
        };
        let mut projects_changed = false;
        for discovered in result.discovered {
            let key = path_key(&discovered.path);
            if self.record_index.contains_key(&key) {
                continue;
            }
            let mut record = ProjectRecord::new(discovered.path, discovered.origin);
            record.last_opened = discovered.last_open_time;
            self.record_index.insert(key, self.records.len());
            self.records.push(record);
            projects_changed = true;
        }
        for inspection in result.inspections {
            if let Some(record) = self.record_mut(&inspection.key) {
                projects_changed |= apply_inspection(record, &inspection);
            }
        }
        if result.imported_recent && !self.settings.engine_recent_imported {
            self.settings.engine_recent_imported = true;
            self.save_settings(cx);
        }
        self.projects_refreshing = false;
        self.rebuild_rows();
        if projects_changed {
            self.save_projects(cx);
        }
        if engines_changed {
            self.save_engines(cx);
        }
        self.rebuild_watcher(cx);
        if !self.running_restored {
            self.running_restored = true;
            self.restore_running_editors(cx);
        }
        cx.notify();
        if let Some(queued) = self.queued_refresh.take() {
            self.refresh(queued, cx);
        }
    }

    pub fn rescan_engines(&mut self, cx: &mut Context<Self>) {
        self.refresh(
            RefreshScope {
                detect_engines: true,
                import_recent: false,
            },
            cx,
        );
    }

    pub fn toggle_favorite(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(record) = self.record_mut(key) else {
            return;
        };
        record.favorite = !record.favorite;
        self.rebuild_rows();
        self.save_projects(cx);
        cx.notify();
    }

    fn insert_inspections(
        &mut self,
        inspections: Vec<ProjectInspection>,
        origin: ProjectOrigin,
        cx: &mut Context<Self>,
    ) -> (usize, Option<String>) {
        let mut added = 0;
        let mut last_key = None;
        for inspection in inspections {
            last_key = Some(inspection.key.clone());
            if let Some(record) = self.record_mut(&inspection.key) {
                apply_inspection(record, &inspection);
                continue;
            }
            let mut record = ProjectRecord::new(inspection.path.clone(), origin);
            apply_inspection(&mut record, &inspection);
            self.record_index
                .insert(inspection.key.clone(), self.records.len());
            self.records.push(record);
            added += 1;
        }
        self.rebuild_rows();
        self.save_projects(cx);
        self.rebuild_watcher(cx);
        (added, last_key)
    }

    pub fn register_created_project(
        &mut self,
        project_file: PathBuf,
        launch: bool,
        cx: &mut Context<Self>,
    ) {
        let work = cx
            .background_spawn(async move { projects::inspect_project(&clean_path(&project_file)) });
        cx.spawn(async move |this, cx| {
            let inspection = work.await;
            this.update(cx, |state, cx| {
                let (_, key) =
                    state.insert_inspections(vec![inspection], ProjectOrigin::Created, cx);
                if let Some(key) = key {
                    cx.emit(HubEvent::RevealProject(key.clone()));
                    if launch {
                        state.launch_project(&key, cx);
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    pub fn add_project_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let work = cx.background_spawn(async move {
            projects::collect_dropped_projects(&paths)
                .iter()
                .map(|path| projects::inspect_project(path))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let inspections = work.await;
            this.update(cx, |state, cx| state.apply_added_projects(inspections, cx))
                .log_err();
        })
        .detach();
    }

    fn apply_added_projects(
        &mut self,
        inspections: Vec<ProjectInspection>,
        cx: &mut Context<Self>,
    ) {
        if inspections.is_empty() {
            self.toast(
                "추가할 .uproject 파일을 찾지 못했습니다",
                ToastKind::Error,
                cx,
            );
            return;
        }
        let (added, reveal_key) = self.insert_inspections(inspections, ProjectOrigin::Manual, cx);
        if added == 0 {
            self.toast("이미 목록에 있는 프로젝트입니다", ToastKind::Info, cx);
        } else {
            self.toast(
                format!("프로젝트 {added}개를 추가했습니다"),
                ToastKind::Success,
                cx,
            );
        }
        if let Some(key) = reveal_key {
            cx.emit(HubEvent::RevealProject(key));
        }
        cx.notify();
    }

    pub fn launch_project(&mut self, key: &str, cx: &mut Context<Self>) -> LaunchOutcome {
        let Some(row) = self.row(key).cloned() else {
            return LaunchOutcome::Unavailable;
        };
        if row.presence != ProjectPresence::Present {
            self.toast("프로젝트 경로에 접근할 수 없습니다", ToastKind::Error, cx);
            return LaunchOutcome::Unavailable;
        }
        if let Some(pid) = self.running_pid_for_project(key) {
            let focus =
                cx.background_spawn(async move { platform::bring_process_window_to_front(pid) });
            cx.spawn(async move |this, cx| {
                if !focus.await {
                    this.update(cx, |state, cx| {
                        state.toast("이미 실행 중입니다", ToastKind::Info, cx)
                    })
                    .log_err();
                }
            })
            .detach();
            return LaunchOutcome::AlreadyRunning;
        }
        let Some(resolution) = row.resolution.as_ref() else {
            self.toast(
                "프로젝트 정보를 읽는 중입니다. 잠시 후 다시 시도하세요",
                ToastKind::Info,
                cx,
            );
            return LaunchOutcome::Unavailable;
        };
        let Some(engine) = resolution
            .engine_key()
            .and_then(|engine_key| self.engine(engine_key))
            .cloned()
        else {
            return LaunchOutcome::EngineMissing;
        };
        self.start_editor(Some((row.key.clone(), row.path.clone())), engine, cx);
        LaunchOutcome::Launching
    }

    pub fn launch_engine(&mut self, engine_key: &str, cx: &mut Context<Self>) {
        match self.engine(engine_key).cloned() {
            Some(engine) if engine.available => self.start_editor(None, engine, cx),
            _ => self.toast("엔진 경로를 찾을 수 없습니다", ToastKind::Error, cx),
        }
    }

    fn start_editor(
        &mut self,
        project: Option<(String, PathBuf)>,
        engine: Engine,
        cx: &mut Context<Self>,
    ) {
        let engine_root = engine.root.clone();
        let project_path = project.as_ref().map(|(_, path)| path.clone());
        let spawn = cx.background_spawn(async move {
            platform::spawn_editor(&engine_root, project_path.as_deref())
        });
        let project_key = project.map(|(key, _)| key);
        let engine_key = engine.key.clone();
        cx.spawn(async move |this, cx| {
            let result = spawn.await;
            this.update(cx, |state, cx| match result {
                Ok(child) => state.track_child(child, project_key, engine_key, cx),
                Err(error) => {
                    log::error!("{error:#}");
                    state.toast(format!("엔진 실행 실패: {error:#}"), ToastKind::Error, cx);
                }
            })
            .log_err();
        })
        .detach();
    }

    fn track_child(
        &mut self,
        mut child: std::process::Child,
        project_key: Option<String>,
        engine_key: String,
        cx: &mut Context<Self>,
    ) {
        let pid = child.id();
        if let Some(key) = project_key.as_deref()
            && let Some(record) = self.record_mut(key)
        {
            record.last_opened = Some(unix_now());
            self.rebuild_rows();
            self.save_projects(cx);
        }
        self.running.insert(
            pid,
            RunningProcess {
                project_key,
                engine_key: Some(engine_key),
            },
        );
        self.track_exit(
            pid,
            move || {
                child
                    .wait()
                    .map(|_| ())
                    .context("waiting for editor failed")
            },
            cx,
        );
        cx.emit(HubEvent::EditorLaunched);
        cx.notify();
    }

    fn track_exit(
        &mut self,
        pid: u32,
        wait: impl FnOnce() -> Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let (sender, receiver) = oneshot::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name(format!("editor-wait-{pid}"))
            .spawn(move || {
                if let Err(error) = wait() {
                    log::warn!("Editor process {pid}: {error:#}");
                }
                if sender.send(()).is_err() {
                    log::debug!("Editor exit for {pid} was not observed");
                }
            });
        if let Err(error) = spawned {
            log::error!("Cannot watch editor process {pid}: {error}");
            self.running.remove(&pid);
            return;
        }
        self.process_waits.insert(
            pid,
            cx.spawn(async move |this, cx| {
                if receiver.await.is_err() {
                    log::debug!("Editor wait channel for {pid} closed");
                }
                this.update(cx, |state, cx| state.process_exited(pid, cx))
                    .log_err();
            }),
        );
    }

    fn process_exited(&mut self, pid: u32, cx: &mut Context<Self>) {
        self.running.remove(&pid);
        self.process_waits.remove(&pid);
        cx.notify();
    }

    fn restore_running_editors(&mut self, cx: &mut Context<Self>) {
        let work = cx.background_spawn(async move {
            platform::find_running_editors()
                .into_iter()
                .map(|editor| (editor.pid, path_key(&clean_path(&editor.project))))
                .collect::<Vec<_>>()
        });
        self.restore_running_task = Some(cx.spawn(async move |this, cx| {
            let found = work.await;
            this.update(cx, |state, cx| {
                for (pid, key) in found {
                    if state.running.contains_key(&pid) {
                        continue;
                    }
                    let engine_key = state
                        .row(&key)
                        .and_then(|row| row.resolution.as_ref())
                        .and_then(|resolution| resolution.engine_key())
                        .map(str::to_string);
                    state.running.insert(
                        pid,
                        RunningProcess {
                            project_key: Some(key),
                            engine_key,
                        },
                    );
                    state.track_exit(pid, move || platform::wait_for_process_exit(pid), cx);
                }
                cx.notify();
            })
            .log_err();
        }));
    }

    pub fn change_project_engine(
        &mut self,
        project_key: &str,
        engine_key: &str,
        create_backup: bool,
        launch_after: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(row) = self.row(project_key).cloned() else {
            return Task::ready(Err(anyhow!("프로젝트를 찾을 수 없습니다")));
        };
        let Some(engine) = self.engine(engine_key).cloned() else {
            return Task::ready(Err(anyhow!("엔진을 찾을 수 없습니다")));
        };
        if self.is_project_running(project_key) {
            return Task::ready(Err(anyhow!("프로젝트가 실행 중이라 변경할 수 없습니다")));
        }
        let association = engine.identifier.clone();
        let project_path = row.path.clone();
        let work_engine = engine.clone();
        let work = cx.background_spawn(async move {
            if work_engine.source == EngineSource::Manual
                && !platform::is_source_build_registered(&work_engine.identifier)
            {
                platform::register_source_build(&work_engine.identifier, &work_engine.root)?;
            }
            projects::write_uproject_association(&project_path, &association, create_backup)?;
            Ok::<_, anyhow::Error>(association)
        });
        let project_key = project_key.to_string();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |state, cx| match result {
                Ok(association) => {
                    if let Some(record) = state.record_mut(&project_key) {
                        record.engine_association = Some(association);
                    }
                    state.rebuild_rows();
                    state.save_projects(cx);
                    state.toast(
                        format!(
                            "{}의 엔진을 {}(으)로 변경했습니다",
                            row.name,
                            engine.label()
                        ),
                        ToastKind::Success,
                        cx,
                    );
                    if launch_after {
                        state.launch_project(&project_key, cx);
                    }
                    cx.notify();
                    Ok(())
                }
                Err(error) => {
                    log::error!("{error:#}");
                    state.toast(format!("엔진 변경 실패: {error:#}"), ToastKind::Error, cx);
                    Err(error)
                }
            })?
        })
    }

    pub fn add_engine_location(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let work = cx.background_spawn(async move {
            let root = clean_path(&root);
            if !engines::is_engine_root(&root) {
                bail!("엔진 폴더가 아닙니다 (Engine\\Build\\Build.version 없음)");
            }
            Ok(root)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |state, cx| match result {
                Ok(root) => {
                    let key = path_key(&root);
                    let already_known = state.engines.iter().any(|engine| engine.key == key)
                        || state
                            .manual_engines
                            .iter()
                            .any(|engine| path_key(&engine.root) == key);
                    if already_known {
                        state.toast("이미 등록된 엔진입니다", ToastKind::Info, cx);
                        return;
                    }
                    state.manual_engines.push(ManualEngine {
                        identifier: new_engine_identifier(),
                        root,
                    });
                    state.save_engines(cx);
                    state.rescan_engines(cx);
                    state.toast("엔진을 추가했습니다", ToastKind::Success, cx);
                }
                Err(error) => state.toast(format!("{error:#}"), ToastKind::Error, cx),
            })
            .log_err();
        })
        .detach();
    }

    pub fn remove_engine(
        &mut self,
        engine_key: &str,
        removal: EngineRemoval,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(engine) = self.engine(engine_key).cloned() else {
            return Task::ready(Err(anyhow!("엔진을 찾을 수 없습니다")));
        };
        if engine.source == EngineSource::Launcher {
            return Task::ready(Err(anyhow!(
                "Launcher로 설치된 엔진은 Epic Games Launcher에서 제거해야 합니다"
            )));
        }
        if self.is_engine_running(engine_key) {
            return Task::ready(Err(anyhow!("엔진이 실행 중이라 제거할 수 없습니다")));
        }
        let work_engine = engine.clone();
        let work = cx.background_spawn(async move {
            if work_engine.source == EngineSource::SourceBuild {
                platform::unregister_source_build(&work_engine.identifier)?;
            }
            if removal == EngineRemoval::DeleteFolder && work_engine.root.exists() {
                trash::delete(&work_engine.root)
                    .context("엔진 폴더를 휴지통으로 이동하지 못했습니다")?;
            }
            Ok::<_, anyhow::Error>(())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |state, cx| {
                if result.is_ok() && engine.source == EngineSource::Manual {
                    state
                        .manual_engines
                        .retain(|manual| path_key(&manual.root) != engine.key);
                    state.save_engines(cx);
                }
                state.rescan_engines(cx);
                match result {
                    Ok(()) => {
                        let message = match removal {
                            EngineRemoval::UnregisterOnly => {
                                format!(
                                    "엔진 {}을(를) 목록에서 제거했습니다",
                                    engine.version_label()
                                )
                            }
                            EngineRemoval::DeleteFolder => {
                                format!(
                                    "엔진 {} 폴더를 휴지통으로 이동했습니다",
                                    engine.version_label()
                                )
                            }
                        };
                        state.toast(message, ToastKind::Success, cx);
                        Ok(())
                    }
                    Err(error) => {
                        log::error!("{error:#}");
                        state.toast(format!("엔진 제거 실패: {error:#}"), ToastKind::Error, cx);
                        Err(error)
                    }
                }
            })?
        })
    }

    pub fn ensure_engine_sizes(&mut self, cx: &mut Context<Self>) {
        if self.size_task.is_some() {
            return;
        }
        let targets: Vec<(String, PathBuf, Option<EngineSizeRecord>)> = self
            .engines
            .iter()
            .filter(|engine| engine.available)
            .map(|engine| {
                let cached = self
                    .size_records
                    .iter()
                    .find(|record| record.key == engine.key)
                    .cloned();
                (engine.key.clone(), engine.root.clone(), cached)
            })
            .collect();
        for (key, _, cached) in &targets {
            if cached.is_none() {
                self.engine_sizes
                    .insert(key.clone(), EngineSizeState::Calculating);
            }
        }
        cx.notify();
        self.size_task = Some(cx.spawn(async move |this, cx| {
            for (key, root, cached) in targets {
                let measured = cx
                    .background_spawn(async move {
                        let modified = engines::folder_modified(&root);
                        match cached {
                            Some(cached)
                                if cached.modified.is_some() && cached.modified == modified =>
                            {
                                cached
                            }
                            _ => EngineSizeRecord {
                                key,
                                modified,
                                bytes: engines::folder_size(&root),
                            },
                        }
                    })
                    .await;
                let applied = this.update(cx, |state, cx| {
                    state
                        .engine_sizes
                        .insert(measured.key.clone(), EngineSizeState::Known(measured.bytes));
                    state
                        .size_records
                        .retain(|record| record.key != measured.key);
                    state.size_records.push(measured);
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
            this.update(cx, |state, cx| {
                state.size_task = None;
                state.save_engines(cx);
            })
            .log_err();
        }));
    }

    pub fn open_epic_launcher(&mut self, cx: &mut Context<Self>) {
        let configured = self.settings.epic_launcher_path.clone();
        let work = cx.background_spawn(async move {
            let launcher = configured
                .filter(|path| path.is_file())
                .or_else(platform::detect_epic_launcher);
            match launcher {
                Some(path) => platform::spawn_program(&path).map(|()| true),
                None => Ok(false),
            }
        });
        cx.spawn(async move |this, cx| match work.await {
            Ok(true) => {}
            Ok(false) => {
                cx.update(|cx| cx.open_url("com.epicgames.launcher://"))
                    .log_err();
            }
            Err(error) => {
                this.update(cx, |state, cx| {
                    state.toast(
                        format!("Epic Launcher 실행 실패: {error:#}"),
                        ToastKind::Error,
                        cx,
                    )
                })
                .log_err();
            }
        })
        .detach();
    }

    fn compute_watch_plan(&self) -> WatchPlan {
        let mut plan = WatchPlan {
            recursive_roots: self.settings.watch_folders.iter().cloned().collect(),
            directories: BTreeSet::new(),
            launcher_directory: platform::launcher_installed_dat_path()
                .and_then(|path| path.parent().map(Path::to_path_buf)),
        };
        for record in &self.records {
            if record.presence != ProjectPresence::Present {
                continue;
            }
            let covered = plan
                .recursive_roots
                .iter()
                .any(|root| record.path.starts_with(root));
            if covered {
                continue;
            }
            if let Some(project_dir) = record.path.parent() {
                plan.directories.insert(project_dir.to_path_buf());
                if let Some(parent) = project_dir.parent() {
                    plan.directories.insert(parent.to_path_buf());
                }
            }
        }
        plan
    }

    fn rebuild_watcher(&mut self, cx: &mut Context<Self>) {
        let plan = self.compute_watch_plan();
        if plan == self.watch_plan && self.watcher_task.is_some() {
            return;
        }
        self.watch_plan = plan.clone();
        let (sender, receiver) = mpsc::unbounded::<WatchSignal>();
        let classifier_plan = plan.clone();
        let create =
            cx.background_spawn(async move { create_watcher(plan, classifier_plan, sender) });
        self.watcher_task = Some(cx.spawn(async move |this, cx| match create.await {
            Ok(watcher) => {
                this.update(cx, |state, _| state.watcher = Some(watcher))
                    .log_err();
            }
            Err(error) => log::error!("Cannot start file watcher: {error:#}"),
        }));
        self.watch_events_task = Some(cx.spawn(async move |this, cx| {
            watch_loop(this, receiver, cx).await;
        }));
    }
}

fn apply_inspection(record: &mut ProjectRecord, inspection: &ProjectInspection) -> bool {
    let before = record.clone();
    record.presence = inspection.presence;
    if let Some(info) = &inspection.info {
        record.engine_association = Some(info.engine_association.clone());
        record.is_cpp = info.is_cpp;
    }
    *record != before
}

fn gather_refresh(
    scope: RefreshScope,
    manual_engines: &[ManualEngine],
    record_paths: &[PathBuf],
    watch_folders: &[PathBuf],
) -> RefreshResult {
    let engines = scope
        .detect_engines
        .then(|| engines::detect_engines(manual_engines));
    let mut discovered = Vec::new();
    for folder in watch_folders {
        discovered.extend(
            projects::find_uprojects(folder, WATCH_FOLDER_DEPTH)
                .iter()
                .map(|path| DiscoveredProject {
                    path: clean_path(path),
                    origin: ProjectOrigin::WatchFolder,
                    last_open_time: None,
                }),
        );
    }
    for root in projects::read_epic_created_project_roots() {
        discovered.extend(
            projects::find_uprojects(&root, EPIC_CREATED_ROOT_DEPTH)
                .iter()
                .map(|path| DiscoveredProject {
                    path: clean_path(path),
                    origin: ProjectOrigin::EpicLauncher,
                    last_open_time: None,
                }),
        );
    }
    if scope.import_recent {
        for recent in projects::read_engine_recent_projects() {
            if recent.path.is_file() {
                discovered.push(DiscoveredProject {
                    path: clean_path(&recent.path),
                    origin: ProjectOrigin::EngineRecent,
                    last_open_time: recent.last_open_time,
                });
            }
        }
    }
    let mut seen = HashSet::new();
    let mut unique_discovered = Vec::new();
    for project in discovered {
        if seen.insert(path_key(&project.path)) {
            unique_discovered.push(project);
        }
    }
    let mut inspections = Vec::new();
    let mut inspected = HashSet::new();
    for path in record_paths
        .iter()
        .chain(unique_discovered.iter().map(|project| &project.path))
    {
        if inspected.insert(path_key(path)) {
            inspections.push(projects::inspect_project(path));
        }
    }
    RefreshResult {
        engines,
        discovered: unique_discovered,
        inspections,
        imported_recent: scope.import_recent,
    }
}

fn create_watcher(
    plan: WatchPlan,
    classifier_plan: WatchPlan,
    sender: mpsc::UnboundedSender<WatchSignal>,
) -> Result<notify::RecommendedWatcher> {
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                if let Some(signal) = classify_event(&classifier_plan, &event)
                    && sender.unbounded_send(signal).is_err()
                {
                    log::debug!("File watch receiver closed");
                }
            }
            Err(error) => log::warn!("File watch error: {error}"),
        })
        .context("Cannot create file watcher")?;
    for root in &plan.recursive_roots {
        if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
            log::warn!("Cannot watch {}: {error}", root.display());
        }
    }
    for directory in plan
        .directories
        .iter()
        .chain(plan.launcher_directory.iter())
    {
        if let Err(error) = watcher.watch(directory, RecursiveMode::NonRecursive) {
            log::debug!("Cannot watch {}: {error}", directory.display());
        }
    }
    Ok(watcher)
}

fn classify_event(plan: &WatchPlan, event: &notify::Event) -> Option<WatchSignal> {
    if matches!(event.kind, EventKind::Access(_)) {
        return None;
    }
    let mut signal = None;
    for path in &event.paths {
        let is_launcher_file = path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("LauncherInstalled.dat"));
        if is_launcher_file {
            return Some(WatchSignal::Engines);
        }
        if is_uproject(path) || plan.directories.contains(path) {
            signal = Some(WatchSignal::Projects);
            continue;
        }
        let within_scan_depth = plan.recursive_roots.iter().any(|root| {
            path.strip_prefix(root).is_ok_and(|relative| {
                relative.components().count() <= WATCH_FOLDER_DEPTH + 1
                    && !relative.components().any(|component| {
                        projects::is_skipped_directory(&component.as_os_str().to_string_lossy())
                    })
            })
        });
        if within_scan_depth {
            signal = Some(WatchSignal::Projects);
        }
    }
    signal
}

async fn watch_loop(
    this: WeakEntity<HubState>,
    mut receiver: mpsc::UnboundedReceiver<WatchSignal>,
    cx: &mut AsyncApp,
) {
    while let Some(first) = receiver.next().await {
        let mut scope = RefreshScope {
            detect_engines: first == WatchSignal::Engines,
            import_recent: false,
        };
        cx.background_executor()
            .timer(Duration::from_millis(600))
            .await;
        while let Ok(signal) = receiver.try_recv() {
            scope.detect_engines |= signal == WatchSignal::Engines;
        }
        if this
            .update(cx, |state, cx| state.refresh(scope, cx))
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(root: &Path) -> WatchPlan {
        WatchPlan {
            recursive_roots: [root.to_path_buf()].into_iter().collect(),
            directories: [root.join("Elsewhere")].into_iter().collect(),
            launcher_directory: None,
        }
    }

    fn event(kind: EventKind, path: PathBuf) -> notify::Event {
        notify::Event::new(kind).add_path(path)
    }

    #[test]
    fn classifies_watch_events() {
        let root = PathBuf::from("/watched");
        let plan = plan(&root);
        let create = EventKind::Create(notify::event::CreateKind::Any);
        assert_eq!(
            classify_event(&plan, &event(create, root.join("a/b/c/d/Deep.uproject"))),
            Some(WatchSignal::Projects)
        );
        assert_eq!(
            classify_event(&plan, &event(create, root.join("NewProject"))),
            Some(WatchSignal::Projects)
        );
        assert_eq!(
            classify_event(
                &plan,
                &event(create, root.join("Game/Intermediate/file.obj"))
            ),
            None
        );
        assert_eq!(
            classify_event(&plan, &event(create, root.join("a/b/c/d/e/f.txt"))),
            None
        );
        assert_eq!(
            classify_event(
                &plan,
                &event(create, PathBuf::from("/data/LauncherInstalled.dat"))
            ),
            Some(WatchSignal::Engines)
        );
        assert_eq!(
            classify_event(
                &plan,
                &event(
                    EventKind::Access(notify::event::AccessKind::Any),
                    root.join("Game.uproject")
                )
            ),
            None
        );
    }

    #[test]
    fn inspection_updates_presence_and_association() {
        let mut record =
            ProjectRecord::new(PathBuf::from("/p/Game.uproject"), ProjectOrigin::Manual);
        record.favorite = true;
        let inspection = ProjectInspection {
            key: record.key(),
            path: record.path.clone(),
            presence: ProjectPresence::Present,
            info: Some(crate::unreal::UProjectInfo {
                engine_association: "5.8".into(),
                is_cpp: true,
            }),
        };
        assert!(apply_inspection(&mut record, &inspection));
        assert_eq!(record.engine_association.as_deref(), Some("5.8"));
        assert!(record.is_cpp);
        assert!(record.favorite);
        assert!(!apply_inspection(&mut record, &inspection));

        let gone = ProjectInspection {
            presence: ProjectPresence::Gone,
            info: None,
            ..inspection
        };
        assert!(apply_inspection(&mut record, &gone));
        assert_eq!(record.presence, ProjectPresence::Gone);
        assert_eq!(record.engine_association.as_deref(), Some("5.8"));
    }
}
