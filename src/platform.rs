#[cfg(not(target_os = "windows"))]
mod fallback;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(not(target_os = "windows"))]
pub use fallback::*;
#[cfg(target_os = "windows")]
pub use windows::*;

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub const AUTOSTART_ARGUMENT: &str = "--autostart";

pub fn editor_executable(engine_root: &Path) -> Option<PathBuf> {
    editor_binary_candidates(engine_root)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

pub fn spawn_editor(engine_root: &Path, project: Option<&Path>) -> Result<Child> {
    let executable = editor_executable(engine_root).ok_or_else(|| {
        tr_anyhow!(
            "에디터 실행 파일을 찾을 수 없습니다: {}",
            "Editor executable not found: {}",
            engine_root.display()
        )
    })?;
    let mut command = Command::new(&executable);
    if let Some(project) = project {
        command.arg(project);
    }
    if let Some(directory) = executable.parent() {
        command.current_dir(directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached(&mut command);
    command.spawn().with_context(|| {
        trf!(
            "{} 실행에 실패했습니다",
            "Failed to start {}",
            executable.display()
        )
    })
}

pub fn spawn_program(executable: &Path) -> Result<()> {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached(&mut command);
    command.spawn().map(|_child| ()).with_context(|| {
        trf!(
            "{} 실행에 실패했습니다",
            "Failed to start {}",
            executable.display()
        )
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningEditor {
    pub pid: u32,
    pub project: PathBuf,
}

pub fn find_running_editors() -> Vec<RunningEditor> {
    let mut system = match System::new() {
        Ok(system) => system,
        Err(error) => {
            log::warn!("Cannot enumerate processes: {error}");
            return Vec::new();
        }
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    system
        .processes()
        .iter()
        .filter(|(_, process)| is_editor_process_name(&process.name().to_string_lossy()))
        .filter_map(|(pid, process)| {
            let project = process.cmd().iter().skip(1).find_map(|argument| {
                let text = argument.to_string_lossy();
                let trimmed = text.trim().trim_matches('"');
                let path = PathBuf::from(trimmed);
                crate::unreal::is_uproject(&path).then_some(path)
            })?;
            Some(RunningEditor {
                pid: pid.as_u32(),
                project,
            })
        })
        .collect()
}

pub fn is_editor_process_name(name: &str) -> bool {
    let lowercase = name.to_lowercase();
    let stem = lowercase.strip_suffix(".exe").unwrap_or(&lowercase);
    stem == "unrealeditor" || stem == "ue4editor"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_editor_process_names() {
        assert!(is_editor_process_name("UnrealEditor.exe"));
        assert!(is_editor_process_name("UE4Editor.exe"));
        assert!(is_editor_process_name("UnrealEditor"));
        assert!(!is_editor_process_name("UnrealEditor-Cmd.exe"));
        assert!(!is_editor_process_name("EpicGamesLauncher.exe"));
    }
}
