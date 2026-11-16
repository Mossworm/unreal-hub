use crate::engines::Engine;
use crate::project_build::{self, BuildOutcome};
use crate::state::{HubState, ToastKind};
use crate::toolchain::{
    self, ComponentState, InstallOutcome, InstallPlan, InstallProgress, ToolchainRequirements,
    ToolchainStatus, VisualStudioPlan,
};
use crate::ui::components::{
    Button, Icon, dialog_body, dialog_footer, dialog_frame, error_note, icon, section_label,
    spinner_text, warning_note,
};
use crate::ui::dialogs::DialogEvent;
use crate::ui::theme::Theme;
use crate::util::{ResultExt, format_bytes};
use anyhow::Result;
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, IntoElement, Render, Task, Window, div,
    prelude::*, rems,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const VISIBLE_LOG_LINES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckReason {
    BeforeBuild,
    AfterInstall { reboot_required: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Checking,
    Unavailable(String),
    NeedsTools,
    Installing(InstallProgress),
    InstallFailed(String),
    ToolsReady {
        reboot_required: bool,
    },
    Building,
    Succeeded,
    Failed {
        message: String,
        toolchain_problem: bool,
    },
}

pub struct BuildDialog {
    state: Entity<HubState>,
    project_key: String,
    project_name: String,
    project_file: PathBuf,
    engine: Option<Engine>,
    requirements: ToolchainRequirements,
    status: Option<ToolchainStatus>,
    plan: Option<InstallPlan>,
    phase: Phase,
    install_cancelled: bool,
    lines: VecDeque<String>,
    progress: Option<(u32, u32)>,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

impl EventEmitter<DialogEvent> for BuildDialog {}

impl Drop for BuildDialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

type WorkerChannels<P, R> = (mpsc::UnboundedReceiver<P>, oneshot::Receiver<R>);

fn spawn_worker<P, R>(
    name: &str,
    work: impl FnOnce(&mut dyn FnMut(P)) -> R + Send + 'static,
) -> Result<WorkerChannels<P, R>>
where
    P: Send + 'static,
    R: Send + 'static,
{
    let (progress_sender, progress_receiver) = mpsc::unbounded::<P>();
    let (result_sender, result_receiver) = oneshot::channel::<R>();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let result = work(&mut |update| {
                if progress_sender.unbounded_send(update).is_err() {
                    log::debug!("Worker progress receiver closed");
                }
            });
            drop(progress_sender);
            if result_sender.send(result).is_err() {
                log::debug!("Worker finished after the dialog closed");
            }
        })?;
    Ok((progress_receiver, result_receiver))
}

impl BuildDialog {
    pub fn new(state: Entity<HubState>, project_key: String, cx: &mut Context<Self>) -> Self {
        let hub = state.read(cx);
        let row = hub.row(&project_key).cloned();
        let engine = row
            .as_ref()
            .and_then(|row| row.resolution.as_ref())
            .and_then(|resolution| resolution.engine_key())
            .and_then(|key| hub.engine(key))
            .cloned();
        let unavailable = match &row {
            None => Some(tr!("프로젝트를 찾을 수 없습니다.", "Project not found.")),
            Some(row) if !row.is_cpp => Some(tr!(
                "C++ 프로젝트가 아니어서 빌드할 코드가 없습니다.",
                "This is not a C++ project, so there is no code to build."
            )),
            Some(_) if engine.is_none() => Some(tr!(
                "이 프로젝트의 엔진을 찾을 수 없습니다. 먼저 엔진 버전을 변경하세요.",
                "The engine for this project was not found. Change the engine version first."
            )),
            Some(_) => None,
        };
        let mut dialog = Self {
            state,
            project_key,
            project_name: row.as_ref().map(|row| row.name.clone()).unwrap_or_default(),
            project_file: row.map(|row| row.path).unwrap_or_default(),
            engine,
            requirements: ToolchainRequirements::default(),
            status: None,
            plan: None,
            phase: Phase::Checking,
            install_cancelled: false,
            lines: VecDeque::new(),
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
        };
        match unavailable {
            Some(message) => dialog.phase = Phase::Unavailable(message.to_string()),
            None => dialog.check(CheckReason::BeforeBuild, cx),
        }
        dialog
    }

    fn engine_root(&self) -> Option<PathBuf> {
        self.engine.as_ref().map(|engine| engine.root.clone())
    }

    fn check(&mut self, reason: CheckReason, cx: &mut Context<Self>) {
        let Some(engine_root) = self.engine_root() else {
            return;
        };
        self.phase = Phase::Checking;
        let inspection = cx.background_spawn(async move { toolchain::inspect(&engine_root) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let (requirements, status) = inspection.await;
            this.update(cx, |dialog, cx| {
                dialog.apply_inspection(requirements, status, reason, cx)
            })
            .log_err();
        }));
        cx.notify();
    }

    fn apply_inspection(
        &mut self,
        requirements: ToolchainRequirements,
        status: ToolchainStatus,
        reason: CheckReason,
        cx: &mut Context<Self>,
    ) {
        self.task = None;
        let ready = status.is_ready();
        self.plan = Some(toolchain::plan_install(&requirements, &status));
        self.requirements = requirements;
        self.status = Some(status);
        match (reason, ready) {
            (CheckReason::BeforeBuild, true) => self.start_build(cx),
            (CheckReason::BeforeBuild, false) => self.phase = Phase::NeedsTools,
            (CheckReason::AfterInstall { reboot_required }, true) => {
                self.phase = Phase::ToolsReady { reboot_required }
            }
            (CheckReason::AfterInstall { reboot_required }, false) => {
                self.phase = Phase::InstallFailed(if reboot_required {
                    tr!(
                        "설치는 끝났지만 PC를 다시 시작해야 사용할 수 있습니다. 재부팅한 뒤 다시 빌드하세요.",
                        "Installation finished, but the PC must restart before the tools can be used. Restart and build again."
                    )
                    .to_string()
                } else {
                    tr!(
                        "설치는 끝났지만 필요한 구성 요소를 찾지 못했습니다. 다운로드 페이지에서 직접 설치해 보세요.",
                        "Installation finished, but the required components were not found. Try installing them from the download page."
                    )
                    .to_string()
                })
            }
        }
        cx.notify();
    }

    fn start_build(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).is_project_running(&self.project_key) {
            self.phase = Phase::Failed {
                message: tr!(
                    "프로젝트가 실행 중입니다. 에디터를 닫은 뒤 다시 빌드하세요.",
                    "The project is running. Close the editor and build again."
                )
                .to_string(),
                toolchain_problem: false,
            };
            cx.notify();
            return;
        }
        let Some(engine_root) = self.engine_root() else {
            return;
        };
        let project_file = self.project_file.clone();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.lines.clear();
        self.progress = None;
        self.phase = Phase::Building;
        let worker = spawn_worker("project-build", move |report: &mut dyn FnMut(String)| {
            project_build::build_editor(&engine_root, &project_file, &cancel, &mut |line| {
                report(line.to_string())
            })
        });
        let (mut lines, result) = match worker {
            Ok(channels) => channels,
            Err(error) => {
                self.phase = Phase::Failed {
                    message: trf!(
                        "작업 스레드를 시작하지 못했습니다: {error}",
                        "Failed to start the worker thread: {error}"
                    ),
                    toolchain_problem: false,
                };
                cx.notify();
                return;
            }
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            while let Some(line) = lines.next().await {
                if this
                    .update(cx, |dialog, cx| dialog.push_line(line, cx))
                    .is_err()
                {
                    return;
                }
            }
            let result = result.await.unwrap_or_else(|_| {
                Err(tr_anyhow!(
                    "빌드 작업이 중단되었습니다",
                    "The build was interrupted"
                ))
            });
            this.update(cx, |dialog, cx| dialog.finish_build(result, cx))
                .log_err();
        }));
        cx.notify();
    }

    fn push_line(&mut self, line: String, cx: &mut Context<Self>) {
        if let Some(progress) = project_build::progress_from_line(&line) {
            self.progress = Some(progress);
        }
        self.lines.push_back(line);
        while self.lines.len() > VISIBLE_LOG_LINES {
            self.lines.pop_front();
        }
        cx.notify();
    }

    fn finish_build(&mut self, result: Result<BuildOutcome>, cx: &mut Context<Self>) {
        self.task = None;
        let project_name = self.project_name.clone();
        match result {
            Ok(BuildOutcome::Succeeded) => {
                self.phase = Phase::Succeeded;
                self.state.update(cx, |state, cx| {
                    state.toast(
                        trf!(
                            "'{project_name}' 빌드가 완료되었습니다",
                            "Built '{project_name}'"
                        ),
                        ToastKind::Success,
                        cx,
                    )
                });
            }
            Ok(BuildOutcome::Cancelled) => {
                self.state.update(cx, |state, cx| {
                    state.toast(
                        tr!("빌드를 취소했습니다", "Build cancelled"),
                        ToastKind::Info,
                        cx,
                    )
                });
                cx.emit(DialogEvent::Close);
            }
            Ok(BuildOutcome::Failed {
                message,
                toolchain_problem,
            }) => {
                log::error!("Build of {project_name} failed: {message}");
                self.phase = Phase::Failed {
                    message,
                    toolchain_problem,
                };
            }
            Err(error) => {
                log::error!("Build of {project_name} failed: {error:#}");
                self.phase = Phase::Failed {
                    message: format!("{error:#}"),
                    toolchain_problem: false,
                };
            }
        }
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        self.install_cancelled = false;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.phase = Phase::Installing(plan.first_progress());
        let worker = spawn_worker(
            "build-tools-install",
            move |report: &mut dyn FnMut(InstallProgress)| {
                toolchain::install(&plan, &cancel, report)
            },
        );
        let (mut updates, result) = match worker {
            Ok(channels) => channels,
            Err(error) => {
                self.phase = Phase::InstallFailed(trf!(
                    "작업 스레드를 시작하지 못했습니다: {error}",
                    "Failed to start the worker thread: {error}"
                ));
                cx.notify();
                return;
            }
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            while let Some(progress) = updates.next().await {
                let updated = this.update(cx, |dialog, cx| {
                    dialog.phase = Phase::Installing(progress);
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
            let result = result.await.unwrap_or_else(|_| {
                Err(tr_anyhow!(
                    "설치 작업이 중단되었습니다",
                    "The installation was interrupted"
                ))
            });
            this.update(cx, |dialog, cx| dialog.finish_install(result, cx))
                .log_err();
        }));
        cx.notify();
    }

    fn finish_install(&mut self, result: Result<InstallOutcome>, cx: &mut Context<Self>) {
        self.task = None;
        match result {
            Ok(InstallOutcome::Completed) => {
                self.check(
                    CheckReason::AfterInstall {
                        reboot_required: false,
                    },
                    cx,
                );
                return;
            }
            Ok(InstallOutcome::RebootRequired) => {
                self.check(
                    CheckReason::AfterInstall {
                        reboot_required: true,
                    },
                    cx,
                );
                return;
            }
            Ok(InstallOutcome::Cancelled) => {
                self.install_cancelled = true;
                self.phase = Phase::NeedsTools;
            }
            Ok(InstallOutcome::Failed(code)) => {
                log::error!("Build tools installation failed with {code}");
                self.phase = Phase::InstallFailed(toolchain::failure_message(code));
            }
            Err(_) if self.cancel.load(Ordering::Relaxed) => {
                self.install_cancelled = true;
                self.phase = Phase::NeedsTools;
            }
            Err(error) => {
                log::error!("Build tools installation failed: {error:#}");
                self.phase = Phase::InstallFailed(format!("{error:#}"));
            }
        }
        cx.notify();
    }

    fn cancel_work(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        cx.notify();
    }

    fn show_installer(&mut self, cx: &mut Context<Self>) {
        self.install_cancelled = false;
        self.phase = Phase::NeedsTools;
        cx.notify();
    }

    fn open_editor(&mut self, cx: &mut Context<Self>) {
        let key = self.project_key.clone();
        self.state.update(cx, |state, cx| {
            state.launch_project(&key, cx);
        });
        cx.emit(DialogEvent::Close);
    }

    pub fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        match self.phase.clone() {
            Phase::NeedsTools => self.install(cx),
            Phase::ToolsReady { .. } | Phase::Failed { .. } => self.start_build(cx),
            Phase::Succeeded => self.open_editor(cx),
            Phase::InstallFailed(_) => self.check(CheckReason::BeforeBuild, cx),
            Phase::Unavailable(_) => cx.emit(DialogEvent::Close),
            Phase::Checking | Phase::Installing(_) | Phase::Building => {}
        }
    }

    fn requirement_row(
        &self,
        theme: &Theme,
        label: &'static str,
        state: ComponentState,
        requirement: Option<String>,
    ) -> impl IntoElement {
        let (glyph, color) = if state.is_satisfied() {
            (Icon::Check, theme.success)
        } else {
            (Icon::Close, theme.danger)
        };
        let detail = match state {
            ComponentState::Found(version) => trf!("{version} 설치됨", "{version} installed"),
            ComponentState::Missing => match requirement {
                Some(requirement) => {
                    trf!("{requirement} 이상 필요", "{requirement} or later required")
                }
                None => tr!("필요", "Required").to_string(),
            },
            ComponentState::NotRequired => String::new(),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(icon(glyph, 0.9, color))
            .child(div().flex_1().child(label))
            .child(div().text_xs().text_color(theme.text_muted).child(detail))
    }

    fn plan_description(&self) -> Vec<String> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let dotnet = plan.dotnet.as_ref().map(|dotnet| {
            trf!(
                "Microsoft .NET {} 런타임을 설치합니다. UnrealBuildTool 실행에 필요합니다.",
                "Installs the Microsoft .NET {} runtime, which UnrealBuildTool needs to run.",
                dotnet.channel
            )
        });
        let visual_studio = plan
            .visual_studio
            .as_ref()
            .map(Self::visual_studio_description);
        dotnet.into_iter().chain(visual_studio).collect()
    }

    fn download_page(&self) -> String {
        match &self.plan {
            Some(InstallPlan {
                dotnet: Some(dotnet),
                visual_studio: None,
            }) => dotnet.download_page.clone(),
            _ => toolchain::BUILD_TOOLS_DOWNLOAD_PAGE.to_string(),
        }
    }

    fn visual_studio_description(plan: &VisualStudioPlan) -> String {
        match plan {
            VisualStudioPlan::Modify {
                display_name,
                update_first: true,
                ..
            } => trf!(
                "설치된 {display_name}을(를) 최신 버전으로 업데이트한 뒤 C++ 빌드 도구, Windows SDK, .NET Framework SDK를 추가합니다.",
                "Updates the installed {display_name}, then adds the C++ build tools, Windows SDK and .NET Framework SDK."
            ),
            VisualStudioPlan::Modify { display_name, .. } => trf!(
                "설치된 {display_name}에 C++ 빌드 도구, Windows SDK, .NET Framework SDK를 추가합니다.",
                "Adds the C++ build tools, Windows SDK and .NET Framework SDK to the installed {display_name}."
            ),
            VisualStudioPlan::Fresh { .. } => tr!(
                "Visual Studio 2022 Build Tools를 설치합니다. 편집기 없이 C++ 빌드 도구와 Windows SDK, .NET Framework SDK를 설치하며 수 GB를 내려받아 수십 분 정도 걸릴 수 있습니다.",
                "Installs Visual Studio 2022 Build Tools with the C++ build tools, Windows SDK and .NET Framework SDK, without the IDE. It downloads several GB and can take tens of minutes."
            )
            .to_string(),
        }
    }

    fn render_needs_tools(&self, theme: &Theme) -> AnyElement {
        let compiler = self
            .status
            .as_ref()
            .map_or(ComponentState::Missing, |status| status.compiler);
        let windows_sdk = self
            .status
            .as_ref()
            .map_or(ComponentState::Missing, |status| status.windows_sdk);
        let netfx_sdk = self
            .status
            .as_ref()
            .map_or(ComponentState::NotRequired, |status| status.netfx_sdk);
        let dotnet = self
            .status
            .as_ref()
            .map_or(ComponentState::NotRequired, |status| status.dotnet);
        let fresh = matches!(
            self.plan,
            Some(InstallPlan {
                visual_studio: Some(VisualStudioPlan::Fresh { .. }),
                ..
            })
        );
        let uses_visual_studio_installer = self
            .plan
            .as_ref()
            .is_some_and(|plan| plan.visual_studio.is_some());
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(tr!(
                "C++ 프로젝트를 빌드하려면 아래 도구가 필요합니다.",
                "Building a C++ project requires the tools below."
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(self.requirement_row(
                        theme,
                        tr!("C++ 컴파일러 (MSVC)", "C++ compiler (MSVC)"),
                        compiler,
                        self.requirements.minimum_msvc.map(|version| version.to_string()),
                    ))
                    .child(self.requirement_row(
                        theme,
                        "Windows SDK",
                        windows_sdk,
                        self.requirements.minimum_sdk.map(|version| version.to_string()),
                    ))
                    .when(netfx_sdk != ComponentState::NotRequired, |this| {
                        this.child(self.requirement_row(
                            theme,
                            ".NET Framework SDK",
                            netfx_sdk,
                            Some(toolchain::MINIMUM_NETFX_SDK.to_string()),
                        ))
                    })
                    .when(dotnet != ComponentState::NotRequired, |this| {
                        this.child(self.requirement_row(
                            theme,
                            tr!(".NET 런타임", ".NET runtime"),
                            dotnet,
                            self.requirements
                                .dotnet
                                .map(|requirement| requirement.version.to_string()),
                        ))
                    }),
            )
            .child(section_label(theme, tr!("설치 내용", "What will be installed")))
            .children(self.plan_description())
            .child(warning_note(
                theme,
                if uses_visual_studio_installer {
                    tr!(
                        "설치를 시작하면 Windows 관리자 권한 확인 창이 뜹니다. 진행 상황은 Visual Studio Installer 창에 표시됩니다.",
                        "Windows will ask for administrator permission. Progress is shown in the Visual Studio Installer window."
                    )
                } else {
                    tr!(
                        "설치를 시작하면 Windows 관리자 권한 확인 창이 뜹니다.",
                        "Windows will ask for administrator permission."
                    )
                },
            ))
            .when(fresh, |this| {
                this.child(div().text_xs().text_color(theme.text_muted).child(tr!(
                    "Build Tools 사용 조건은 Visual Studio 라이선스를 따릅니다. 개인, 오픈 소스, 소규모 팀은 Community 조건으로 무료로 쓸 수 있지만 규모가 큰 회사는 유료 라이선스가 필요할 수 있습니다.",
                    "Build Tools follow the Visual Studio license. Individuals, open-source projects and small teams can use them free under the Community terms, but larger companies may need a paid license."
                )))
            })
            .when(self.install_cancelled, |this| {
                this.child(error_note(
                    theme,
                    tr!("설치가 취소되었습니다.", "Installation was cancelled."),
                ))
            })
            .into_any_element()
    }

    fn render_installing(&self, theme: &Theme, progress: InstallProgress) -> AnyElement {
        let message = match progress {
            InstallProgress::Downloading { received, total } => {
                let amount = match total.filter(|total| *total > 0) {
                    Some(total) => format!("{}%", received.saturating_mul(100) / total),
                    None => format_bytes(received),
                };
                trf!(
                    "설치 프로그램을 내려받는 중… {amount}",
                    "Downloading the installer… {amount}"
                )
            }
            InstallProgress::Verifying => tr!(
                "설치 프로그램 서명을 확인하는 중…",
                "Verifying the installer signature…"
            )
            .to_string(),
            InstallProgress::Updating => tr!(
                "Visual Studio를 업데이트하는 중…",
                "Updating Visual Studio…"
            )
            .to_string(),
            InstallProgress::Installing => {
                tr!("빌드 도구를 설치하는 중…", "Installing the build tools…").to_string()
            }
            InstallProgress::InstallingDotnet => {
                tr!(".NET 런타임을 설치하는 중…", "Installing the .NET runtime…").to_string()
            }
        };
        let external = matches!(
            progress,
            InstallProgress::Updating | InstallProgress::Installing
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(spinner_text(theme, message))
            .when(external, |this| {
                this.child(div().text_color(theme.text_muted).child(tr!(
                    "Visual Studio Installer 창에서 진행 상황을 확인하세요. 수십 분 걸릴 수 있으며, 끝나면 자동으로 다시 확인합니다.",
                    "Follow the progress in the Visual Studio Installer window. It can take tens of minutes; the hub checks again when it finishes."
                )))
            })
            .into_any_element()
    }

    fn render_log(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .h(rems(13.0))
            .p_2()
            .overflow_hidden()
            .rounded_md()
            .bg(theme.input)
            .border_1()
            .border_color(theme.border)
            .font_family("Consolas")
            .text_xs()
            .text_color(theme.text_muted)
            .children(
                self.lines
                    .iter()
                    .map(|line| div().truncate().child(line.clone())),
            )
    }

    fn render_body(&self, theme: &Theme) -> AnyElement {
        match &self.phase {
            Phase::Checking => {
                spinner_text(theme, tr!("빌드 환경을 확인하는 중…", "Checking the build environment…"))
                    .into_any_element()
            }
            Phase::Unavailable(message) => error_note(theme, message.clone()).into_any_element(),
            Phase::NeedsTools => self.render_needs_tools(theme),
            Phase::Installing(progress) => self.render_installing(theme, *progress),
            Phase::InstallFailed(message) => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(error_note(theme, message.clone()))
                .child(div().text_color(theme.text_muted).child(tr!(
                    "다시 확인하거나 'Build Tools for Visual Studio 2022' 설치 관리자에서 'C++를 사용한 데스크톱 개발'과 '.NET Framework 4.8 SDK'를 선택하세요.",
                    "Check again, or use the 'Build Tools for Visual Studio 2022' installer to select 'Desktop development with C++' and '.NET Framework 4.8 SDK'."
                )))
                .into_any_element(),
            Phase::ToolsReady { reboot_required } => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon(Icon::Check, 1.0, theme.success))
                        .child(tr!("빌드 도구가 준비되었습니다.", "The build tools are ready.")),
                )
                .when(*reboot_required, |this| {
                    this.child(warning_note(
                        theme,
                        tr!(
                            "설치 프로그램이 재부팅을 권장합니다. 빌드가 실패하면 PC를 다시 시작한 뒤 시도하세요.",
                            "The installer recommends a restart. If the build fails, restart the PC and try again."
                        ),
                    ))
                })
                .into_any_element(),
            Phase::Building => {
                let status = match self.progress {
                    Some((done, total)) => trf!(
                        "컴파일하는 중… {done}/{total}",
                        "Compiling… {done}/{total}"
                    ),
                    None => tr!("빌드하는 중…", "Building…").to_string(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(spinner_text(theme, status))
                    .child(self.render_log(theme))
                    .into_any_element()
            }
            Phase::Succeeded => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon(Icon::Check, 1.0, theme.success))
                        .child(tr!("빌드가 완료되었습니다.", "The build finished.")),
                )
                .child(self.render_log(theme))
                .into_any_element(),
            Phase::Failed {
                message,
                toolchain_problem,
            } => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(error_note(theme, message.clone()))
                .when(*toolchain_problem, |this| {
                    this.child(warning_note(
                        theme,
                        tr!(
                            "빌드 도구가 없거나 이 엔진과 맞지 않는 것 같습니다. 빌드 도구를 설치해 보세요.",
                            "The build tools seem to be missing or incompatible with this engine. Try installing them."
                        ),
                    ))
                })
                .when(!self.lines.is_empty(), |this| this.child(self.render_log(theme)))
                .into_any_element(),
        }
    }

    fn render_footer(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let footer = dialog_footer(theme);
        let close = |label: &'static str| {
            Button::new("build-close", label)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)))
        };
        let download_page = Button::new(
            "build-download-page",
            tr!("다운로드 페이지 열기", "Open Download Page"),
        )
        .ghost()
        .on_click({
            let page = self.download_page();
            move |_, _, cx| cx.open_url(&page)
        });
        let log_path = project_build::build_log_path(&self.project_file);
        match &self.phase {
            Phase::Checking => footer.child(close(tr!("닫기", "Close"))),
            Phase::Unavailable(_) => footer.child(close(tr!("닫기", "Close")).primary()),
            Phase::NeedsTools => footer
                .child(download_page)
                .child(div().flex_1())
                .child(close(tr!("취소", "Cancel")))
                .child(
                    Button::new("build-install", tr!("설치", "Install"))
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                ),
            Phase::Installing(progress) => {
                let cancellable = matches!(progress, InstallProgress::Downloading { .. })
                    && !self.cancel.load(Ordering::Relaxed);
                footer.child(
                    Button::new("build-install-cancel", tr!("취소", "Cancel"))
                        .disabled(!cancellable)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_work(cx))),
                )
            }
            Phase::InstallFailed(_) => footer
                .child(download_page)
                .child(div().flex_1())
                .child(close(tr!("닫기", "Close")))
                .child(
                    Button::new("build-recheck", tr!("다시 확인", "Check Again"))
                        .primary()
                        .on_click(
                            cx.listener(|this, _, _, cx| this.check(CheckReason::BeforeBuild, cx)),
                        ),
                ),
            Phase::ToolsReady { .. } => footer.child(close(tr!("닫기", "Close"))).child(
                Button::new("build-start", tr!("빌드", "Build"))
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| this.start_build(cx))),
            ),
            Phase::Building => {
                let cancelling = self.cancel.load(Ordering::Relaxed);
                footer.child(
                    Button::new(
                        "build-cancel",
                        if cancelling {
                            tr!("취소하는 중…", "Cancelling…")
                        } else {
                            tr!("취소", "Cancel")
                        },
                    )
                    .disabled(cancelling)
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_work(cx))),
                )
            }
            Phase::Succeeded => footer.child(close(tr!("닫기", "Close"))).child(
                Button::new("build-open-editor", tr!("에디터 열기", "Open Editor"))
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| this.open_editor(cx))),
            ),
            Phase::Failed {
                toolchain_problem, ..
            } => footer
                .when_some(log_path.filter(|path| path.is_file()), |this, path| {
                    this.child(
                        Button::new("build-open-log", tr!("로그 열기", "Open Log"))
                            .ghost()
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    )
                })
                .when(*toolchain_problem, |this| {
                    this.child(
                        Button::new(
                            "build-show-installer",
                            tr!("빌드 도구 설치…", "Install Build Tools…"),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.show_installer(cx))),
                    )
                })
                .child(div().flex_1())
                .child(close(tr!("닫기", "Close")))
                .child(
                    Button::new("build-retry", tr!("다시 빌드", "Build Again"))
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| this.start_build(cx))),
                ),
        }
    }
}

impl Render for BuildDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let title = if matches!(
            self.phase,
            Phase::NeedsTools | Phase::Installing(_) | Phase::InstallFailed(_)
        ) {
            tr!("빌드 도구 설치", "Install Build Tools").to_string()
        } else {
            trf!("빌드 — {}", "Build — {}", self.project_name)
        };
        dialog_frame(
            &theme,
            title,
            36.0,
            cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
        )
        .child(dialog_body(&theme).child(self.render_body(&theme)))
        .child(self.render_footer(&theme, cx))
    }
}
