use crate::toolchain::{ToolchainRequirements, ToolchainStatus};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const EDITOR_CONFIG_PLATFORM: &str = "LinuxEditor";
pub const BUILD_PLATFORM: &str = "Linux";
pub const DOTNET_EXECUTABLE: &str = "dotnet";
pub const DOTNET_BUNDLE_ARCHITECTURES: &[&str] = &["linux-x64", "linux"];
pub const DOTNET_RUNTIME_INSTALLER: Option<&str> = None;
pub const DOTNET_ARCHITECTURE_ROOT_VARIABLES: &[&str] = &["DOTNET_ROOT_X64", "DOTNET_ROOT_ARM64"];

pub fn launcher_installed_dat_path() -> Option<PathBuf> {
    None
}

pub fn launcher_registry_installs() -> Vec<(String, PathBuf)> {
    Vec::new()
}

pub fn source_build_registrations() -> Vec<(String, PathBuf)> {
    let Some(config) = dirs::config_dir() else {
        return Vec::new();
    };
    let path = config.join("Epic").join("UnrealEngine").join("Install.ini");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut in_section = false;
    let mut builds = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_section = name.eq_ignore_ascii_case("Installations");
            continue;
        }
        if let (true, Some((identifier, root))) = (in_section, line.split_once('=')) {
            builds.push((identifier.trim().to_string(), PathBuf::from(root.trim())));
        }
    }
    builds
}

pub fn is_source_build_registered(identifier: &str) -> bool {
    let normalized = crate::unreal::normalize_identifier(identifier);
    source_build_registrations()
        .iter()
        .any(|(registered, _)| crate::unreal::normalize_identifier(registered) == normalized)
}

pub fn register_source_build(_identifier: &str, _engine_root: &Path) -> Result<()> {
    tr_bail!(
        "이 플랫폼에서는 엔진 등록을 지원하지 않습니다",
        "Engine registration is not supported on this platform"
    )
}

pub fn unregister_source_build(_identifier: &str) -> Result<()> {
    tr_bail!(
        "이 플랫폼에서는 엔진 등록 해제를 지원하지 않습니다",
        "Engine unregistration is not supported on this platform"
    )
}

pub fn unreal_engine_local_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|config| config.join("Epic").join("UnrealEngine"))
}

pub fn epic_game_user_settings_path() -> Option<PathBuf> {
    None
}

pub fn editor_binary_candidates(engine_root: &Path) -> Vec<PathBuf> {
    vec![
        engine_root
            .join("Engine")
            .join("Binaries")
            .join("Linux")
            .join("UnrealEditor"),
    ]
}

pub fn detect_epic_launcher() -> Option<PathBuf> {
    None
}

pub fn configure_detached(_command: &mut Command) {}

pub fn configure_hidden(_command: &mut Command) {}

pub fn unreal_build_tool(engine_root: &Path) -> Option<PathBuf> {
    let path = engine_root
        .join("Engine")
        .join("Binaries")
        .join("DotNET")
        .join("UnrealBuildTool")
        .join("UnrealBuildTool");
    path.is_file().then_some(path)
}

pub fn system_dotnet_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("DOTNET_ROOT")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    roots.extend(
        [
            "/usr/share/dotnet",
            "/usr/lib/dotnet",
            "/usr/local/share/dotnet",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    roots
}

pub fn has_cpp_toolchain() -> bool {
    true
}

pub fn inspect_toolchain(_requirements: &ToolchainRequirements) -> ToolchainStatus {
    ToolchainStatus::not_required()
}

pub fn visual_studio_setup() -> Option<PathBuf> {
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevatedRun {
    Exited(i32),
    Declined,
}

pub fn run_elevated(
    _executable: &Path,
    _parameters: &str,
    _working_directory: &Path,
) -> Result<ElevatedRun> {
    tr_bail!(
        "이 플랫폼에서는 빌드 도구 설치를 지원하지 않습니다",
        "Installing build tools is not supported on this platform"
    )
}

pub fn verify_microsoft_signature(_path: &Path) -> Result<()> {
    tr_bail!(
        "이 플랫폼에서는 서명 확인을 지원하지 않습니다",
        "Signature verification is not supported on this platform"
    )
}

pub fn wait_for_process_exit(pid: u32) -> Result<()> {
    bail!("waiting for process {pid} is not supported on this platform")
}

pub fn bring_process_window_to_front(_pid: u32) -> bool {
    false
}

pub fn set_launch_at_startup(_enabled: bool, _executable: &Path) -> Result<()> {
    bail!(tr!(
        "이 플랫폼에서는 시작 프로그램 등록을 지원하지 않습니다",
        "Starting with the system is not supported on this platform"
    ))
}

pub fn set_window_visible(_window: &impl raw_window_handle::HasWindowHandle, _visible: bool) {}

pub struct SingleInstance;

pub struct AlreadyRunning;

pub fn acquire_single_instance() -> Result<Option<SingleInstance>, AlreadyRunning> {
    Ok(None)
}

pub fn activate_existing_instance(_title: &str) -> bool {
    false
}
