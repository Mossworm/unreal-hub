use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const EDITOR_CONFIG_PLATFORM: &str = "LinuxEditor";

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
    bail!("이 플랫폼에서는 엔진 등록을 지원하지 않습니다")
}

pub fn unregister_source_build(_identifier: &str) -> Result<()> {
    bail!("이 플랫폼에서는 엔진 등록 해제를 지원하지 않습니다")
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

pub fn has_cpp_toolchain() -> bool {
    true
}

pub fn wait_for_process_exit(pid: u32) -> Result<()> {
    bail!("waiting for process {pid} is not supported on this platform")
}

pub fn bring_process_window_to_front(_pid: u32) -> bool {
    false
}
