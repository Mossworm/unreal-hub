use crate::dotnet;
use crate::platform;
use anyhow::{Context as _, Result};
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const BUILD_LOG: &str = "UnrealHub-Build.log";
const TARGET_SUFFIX: &str = ".target.cs";
const EDITOR_TARGET_TYPE: &str = "TargetType.Editor";
const TOOLCHAIN_FAILURE_MARKERS: [&str; 8] = [
    "must be installed in order to build this target",
    "no valid visual c++ toolchain",
    "unable to find valid c++ toolchain",
    "windows sdk must be installed",
    "could not find a valid windows sdk",
    "could not find netfxsdk install dir",
    "install a version of .net framework sdk",
    "you must install or update .net to run this application",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolExit {
    Succeeded,
    Failed { status: String, tail: String },
    Cancelled,
}

pub fn run_build_tool(
    build_tool: &Path,
    arguments: &[OsString],
    engine_root: &Path,
    log_path: &Path,
    cancel: &AtomicBool,
    on_line: &mut dyn FnMut(&str),
) -> Result<ToolExit> {
    if let Some(directory) = log_path.parent() {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("Cannot create {}", directory.display()))?;
    }
    let log_file = std::fs::File::create(log_path)
        .with_context(|| format!("Cannot create {}", log_path.display()))?;
    let error_log = log_file.try_clone()?;
    let mut log_reader = std::fs::File::open(log_path)
        .with_context(|| format!("Cannot read {}", log_path.display()))?;
    let mut command = Command::new(build_tool);
    command
        .args(arguments)
        .current_dir(engine_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(error_log));
    dotnet::configure_build_tool(&mut command, engine_root);
    platform::configure_hidden(&mut command);
    let mut child = command.spawn().with_context(|| {
        trf!(
            "{} 실행에 실패했습니다",
            "Failed to start {}",
            build_tool.display()
        )
    })?;
    let mut pending = Vec::new();
    loop {
        forward_new_lines(&mut log_reader, &mut pending, on_line);
        if cancel.load(Ordering::Relaxed) {
            if let Err(error) = child.kill() {
                log::warn!("Cannot stop {}: {error}", build_tool.display());
            }
            if let Err(error) = child.wait() {
                log::warn!("Cannot reap {}: {error}", build_tool.display());
            }
            return Ok(ToolExit::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            forward_new_lines(&mut log_reader, &mut pending, on_line);
            if !pending.is_empty() {
                on_line(String::from_utf8_lossy(&pending).trim_end());
            }
            if status.success() {
                return Ok(ToolExit::Succeeded);
            }
            let text = std::fs::read_to_string(log_path).unwrap_or_default();
            return Ok(ToolExit::Failed {
                status: status.to_string(),
                tail: log_tail(&text, 3),
            });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn forward_new_lines(
    reader: &mut std::fs::File,
    pending: &mut Vec<u8>,
    on_line: &mut dyn FnMut(&str),
) {
    if let Err(error) = reader.read_to_end(pending) {
        log::debug!("Cannot read the build log: {error}");
        return;
    }
    while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = pending.drain(..=position).collect();
        let text = String::from_utf8_lossy(&line);
        let trimmed = text.trim_end();
        if !trimmed.is_empty() {
            on_line(trimmed);
        }
    }
}

pub fn log_tail(text: &str, count: usize) -> String {
    let mut lines: Vec<&str> = text
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(count)
        .collect();
    lines.reverse();
    lines.join(" / ")
}

pub fn editor_target_from_sources<'a>(
    sources: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let mut targets: Vec<String> = sources
        .into_iter()
        .filter(|(_, contents)| contents.contains(EDITOR_TARGET_TYPE))
        .filter_map(|(file_name, _)| {
            let lowercase = file_name.to_ascii_lowercase();
            lowercase
                .strip_suffix(TARGET_SUFFIX)
                .and_then(|_| file_name.get(..file_name.len() - TARGET_SUFFIX.len()))
                .map(str::to_string)
        })
        .collect();
    targets.sort();
    targets.into_iter().next()
}

pub fn find_editor_target(project_file: &Path) -> Result<String> {
    let source = project_file
        .parent()
        .map(|directory| directory.join("Source"))
        .ok_or_else(|| tr_anyhow!("프로젝트 폴더를 알 수 없습니다", "Unknown project folder"))?;
    let entries = std::fs::read_dir(&source).with_context(|| {
        trf!(
            "{} 폴더를 읽을 수 없습니다",
            "Cannot read folder {}",
            source.display()
        )
    })?;
    let sources: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(TARGET_SUFFIX) {
                return None;
            }
            match std::fs::read_to_string(entry.path()) {
                Ok(contents) => Some((name, contents)),
                Err(error) => {
                    log::warn!("Cannot read {}: {error}", entry.path().display());
                    None
                }
            }
        })
        .collect();
    editor_target_from_sources(
        sources
            .iter()
            .map(|(name, contents)| (name.as_str(), contents.as_str())),
    )
    .ok_or_else(|| {
        tr_anyhow!(
            "Source 폴더에서 에디터 빌드 대상(*.Target.cs)을 찾지 못했습니다",
            "No editor build target (*.Target.cs) was found in the Source folder"
        )
    })
}

pub fn build_arguments(target: &str, project_file: &Path) -> Vec<OsString> {
    let mut project_argument = OsString::from("-Project=");
    project_argument.push(project_file.as_os_str());
    vec![
        OsString::from(target),
        OsString::from(platform::BUILD_PLATFORM),
        OsString::from("Development"),
        project_argument,
        OsString::from("-WaitMutex"),
    ]
}

pub fn build_log_path(project_file: &Path) -> Option<PathBuf> {
    project_file
        .parent()
        .map(|directory| directory.join("Saved").join("Logs").join(BUILD_LOG))
}

pub fn is_toolchain_failure(log: &str) -> bool {
    let lowercase = log.to_lowercase();
    TOOLCHAIN_FAILURE_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
}

pub fn progress_from_line(line: &str) -> Option<(u32, u32)> {
    let inner = line.trim_start().strip_prefix('[')?;
    let (counts, _) = inner.split_once(']')?;
    let (done, total) = counts.split_once('/')?;
    let done = done.trim().parse().ok()?;
    let total: u32 = total.trim().parse().ok()?;
    (total > 0 && done <= total).then_some((done, total))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildOutcome {
    Succeeded,
    Failed {
        message: String,
        toolchain_problem: bool,
    },
    Cancelled,
}

pub fn build_editor(
    engine_root: &Path,
    project_file: &Path,
    cancel: &AtomicBool,
    on_line: &mut dyn FnMut(&str),
) -> Result<BuildOutcome> {
    let build_tool = platform::unreal_build_tool(engine_root).ok_or_else(|| {
        tr_anyhow!(
            "UnrealBuildTool을 찾을 수 없습니다",
            "UnrealBuildTool not found"
        )
    })?;
    let target = find_editor_target(project_file)?;
    let log_path = build_log_path(project_file)
        .ok_or_else(|| tr_anyhow!("프로젝트 폴더를 알 수 없습니다", "Unknown project folder"))?;
    let exit = run_build_tool(
        &build_tool,
        &build_arguments(&target, project_file),
        engine_root,
        &log_path,
        cancel,
        on_line,
    )?;
    Ok(match exit {
        ToolExit::Succeeded => BuildOutcome::Succeeded,
        ToolExit::Cancelled => BuildOutcome::Cancelled,
        ToolExit::Failed { status, tail } => {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            BuildOutcome::Failed {
                message: trf!(
                    "빌드에 실패했습니다 ({status}): {tail}",
                    "The build failed ({status}): {tail}"
                ),
                toolchain_problem: is_toolchain_failure(&log),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_editor_target_by_target_type() {
        let sources = [
            (
                "MyGame.Target.cs",
                "Type = TargetType.Game; ExtraModuleNames.Add(\"MyGame\");",
            ),
            (
                "MyGameEditor.Target.cs",
                "public MyGameEditorTarget(TargetInfo Target) : base(Target) { Type = TargetType.Editor; }",
            ),
            ("MyGameServer.Target.cs", "Type = TargetType.Server;"),
        ];
        assert_eq!(
            editor_target_from_sources(sources).as_deref(),
            Some("MyGameEditor")
        );
    }

    #[test]
    fn finds_editor_target_with_lowercase_extension() {
        let sources = [("Tool.target.cs", "Type = TargetType.Editor;")];
        assert_eq!(editor_target_from_sources(sources).as_deref(), Some("Tool"));
        assert_eq!(
            editor_target_from_sources([("Game.Target.cs", "Type = TargetType.Game;")]),
            None
        );
    }

    #[test]
    fn finds_editor_target_on_disk() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("Source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("Demo.Target.cs"), "Type = TargetType.Game;").unwrap();
        std::fs::write(
            source.join("DemoEditor.Target.cs"),
            "Type = TargetType.Editor;",
        )
        .unwrap();
        let project_file = project.path().join("Demo.uproject");
        assert_eq!(find_editor_target(&project_file).unwrap(), "DemoEditor");
        let empty = tempfile::tempdir().unwrap();
        assert!(find_editor_target(&empty.path().join("Empty.uproject")).is_err());
    }

    #[test]
    fn builds_arguments_with_spaces_and_non_ascii_paths() {
        let project = Path::new(r"D:\내 프로젝트\Demo\Demo.uproject");
        let arguments = build_arguments("DemoEditor", project);
        assert_eq!(
            arguments.get(3),
            Some(&OsString::from(
                r"-Project=D:\내 프로젝트\Demo\Demo.uproject"
            ))
        );
        assert_eq!(arguments.first(), Some(&OsString::from("DemoEditor")));
        assert_eq!(arguments.get(2), Some(&OsString::from("Development")));
    }

    #[test]
    fn detects_toolchain_failures() {
        assert!(is_toolchain_failure(
            "ERROR: Visual Studio 2022 must be installed in order to build this target."
        ));
        assert!(is_toolchain_failure(
            "No valid Visual C++ toolchain was found (minimum version 14.38.33130)."
        ));
        assert!(is_toolchain_failure(
            "Could not find NetFxSDK install dir; this will prevent SwarmInterface from installing."
        ));
        assert!(is_toolchain_failure(
            "You must install or update .NET to run this application."
        ));
        assert!(!is_toolchain_failure(
            "Using Visual Studio 2022 14.44.35211 toolchain and Windows 10.0.22621.0 SDK\nerror C2065: undeclared identifier"
        ));
    }

    #[test]
    fn parses_progress_lines() {
        assert_eq!(
            progress_from_line("[12/345] Compile [x64] MyActor.cpp"),
            Some((12, 345))
        );
        assert_eq!(
            progress_from_line("  [1/1] Link UnrealEditor-Demo.dll"),
            Some((1, 1))
        );
        assert_eq!(
            progress_from_line("[Upgrade] Using backward-compatible"),
            None
        );
        assert_eq!(progress_from_line("Building DemoEditor..."), None);
    }

    #[test]
    fn keeps_last_non_empty_log_lines() {
        assert_eq!(log_tail("a\n\nb\nc\n\nd\n", 3), "b / c / d");
        assert_eq!(log_tail("", 3), "");
    }

    #[test]
    fn forwards_complete_lines_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("log.txt");
        std::fs::write(&path, "first\r\nsecond\npart").unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();
        let mut pending = Vec::new();
        let mut lines = Vec::new();
        forward_new_lines(&mut reader, &mut pending, &mut |line| {
            lines.push(line.to_string())
        });
        assert_eq!(lines, ["first", "second"]);
        assert_eq!(pending, b"part");
    }
}
