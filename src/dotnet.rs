use crate::platform;
use crate::toolchain::{ComponentState, ToolVersion};
use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const RUNTIME_FRAMEWORK: &str = "Microsoft.NETCore.App";
const USE_SYSTEM_DOTNET_VARIABLE: &str = "UE_USE_SYSTEM_DOTNET";
const DOWNLOAD_BASE_URL: &str = "https://aka.ms/dotnet";
const DOWNLOAD_PAGE_BASE_URL: &str = "https://dotnet.microsoft.com/download/dotnet";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollForward {
    Disable,
    LatestPatch,
    Minor,
    LatestMinor,
    Major,
    LatestMajor,
}

impl RollForward {
    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "disable" => Some(Self::Disable),
            "latestpatch" => Some(Self::LatestPatch),
            "minor" => Some(Self::Minor),
            "latestminor" => Some(Self::LatestMinor),
            "major" => Some(Self::Major),
            "latestmajor" => Some(Self::LatestMajor),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeRequirement {
    pub version: ToolVersion,
    pub roll_forward: RollForward,
}

impl RuntimeRequirement {
    pub fn accepts(&self, installed: ToolVersion) -> bool {
        if installed < self.version {
            return false;
        }
        let same_major = installed.major() == self.version.major();
        let same_minor = same_major && installed.minor() == self.version.minor();
        match self.roll_forward {
            RollForward::Disable => installed == self.version,
            RollForward::LatestPatch => same_minor,
            RollForward::Minor | RollForward::LatestMinor => same_major,
            RollForward::Major | RollForward::LatestMajor => true,
        }
    }

    pub fn channel(&self) -> String {
        format!("{}.{}", self.version.major(), self.version.minor())
    }

    pub fn download_page(&self) -> String {
        format!("{DOWNLOAD_PAGE_BASE_URL}/{}", self.channel())
    }

    fn with_latest_major_roll_forward(self) -> Self {
        Self {
            roll_forward: RollForward::LatestMajor,
            ..self
        }
    }
}

pub fn parse_runtime_config(text: &str) -> Result<Option<RuntimeRequirement>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RuntimeConfigFile {
        #[serde(default)]
        runtime_options: RuntimeOptions,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct RuntimeOptions {
        roll_forward: Option<String>,
        framework: Option<Framework>,
        frameworks: Vec<Framework>,
    }

    #[derive(Deserialize)]
    struct Framework {
        name: String,
        version: String,
    }

    let file: RuntimeConfigFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .context("runtimeconfig.json is not valid JSON")?;
    let options = file.runtime_options;
    let roll_forward = options
        .roll_forward
        .as_deref()
        .and_then(RollForward::parse)
        .unwrap_or(RollForward::Minor);
    Ok(options
        .framework
        .into_iter()
        .chain(options.frameworks)
        .find(|framework| framework.name == RUNTIME_FRAMEWORK)
        .and_then(|framework| ToolVersion::parse(&framework.version))
        .map(|version| RuntimeRequirement {
            version,
            roll_forward,
        }))
}

pub fn runtime_config_path(build_tool: &Path) -> PathBuf {
    build_tool.with_extension("runtimeconfig.json")
}

pub fn read_requirement(engine_root: &Path) -> Result<Option<RuntimeRequirement>> {
    let Some(build_tool) = platform::unreal_build_tool(engine_root) else {
        return Ok(None);
    };
    let path = runtime_config_path(&build_tool);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    parse_runtime_config(&text).with_context(|| format!("Cannot parse {}", path.display()))
}

pub fn installed_runtimes(dotnet_root: &Path) -> Vec<ToolVersion> {
    let directory = dotnet_root.join("shared").join(RUNTIME_FRAMEWORK);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| ToolVersion::parse(&entry.file_name().to_string_lossy()))
        .collect()
}

pub fn best_runtime(
    requirement: &RuntimeRequirement,
    installed: &[ToolVersion],
) -> Option<ToolVersion> {
    installed
        .iter()
        .copied()
        .filter(|version| requirement.accepts(*version))
        .max()
}

pub fn bundled_roots(engine_root: &Path) -> Vec<PathBuf> {
    let bundle = engine_root
        .join("Engine")
        .join("Binaries")
        .join("ThirdParty")
        .join("DotNet");
    let Ok(entries) = std::fs::read_dir(&bundle) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            platform::DOTNET_BUNDLE_ARCHITECTURES
                .iter()
                .map(|architecture| entry.path().join(architecture))
                .find(|root| root.join(platform::DOTNET_EXECUTABLE).is_file())
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledRuntime {
    pub root: PathBuf,
    pub version: ToolVersion,
}

pub fn bundled_runtime(
    engine_root: &Path,
    requirement: &RuntimeRequirement,
) -> Option<BundledRuntime> {
    if std::env::var_os(USE_SYSTEM_DOTNET_VARIABLE).is_some_and(|value| value == "1") {
        return None;
    }
    let relaxed = requirement.with_latest_major_roll_forward();
    bundled_roots(engine_root)
        .into_iter()
        .filter_map(|root| {
            let version = best_runtime(&relaxed, &installed_runtimes(&root))?;
            Some(BundledRuntime { root, version })
        })
        .max_by_key(|runtime| runtime.version)
}

pub fn system_runtime(requirement: &RuntimeRequirement) -> Option<ToolVersion> {
    let installed: Vec<ToolVersion> = platform::system_dotnet_roots()
        .iter()
        .flat_map(|root| installed_runtimes(root))
        .collect();
    best_runtime(requirement, &installed)
}

pub fn inspect(engine_root: &Path) -> (Option<RuntimeRequirement>, ComponentState) {
    let requirement = match read_requirement(engine_root) {
        Ok(requirement) => requirement,
        Err(error) => {
            log::warn!("Cannot read the .NET requirement of UnrealBuildTool: {error:#}");
            None
        }
    };
    let Some(requirement) = requirement else {
        return (None, ComponentState::NotRequired);
    };
    let found = bundled_runtime(engine_root, &requirement)
        .map(|runtime| runtime.version)
        .or_else(|| system_runtime(&requirement));
    (
        Some(requirement),
        found.map_or(ComponentState::Missing, ComponentState::Found),
    )
}

pub fn configure_build_tool(command: &mut Command, engine_root: &Path) {
    let requirement = match read_requirement(engine_root) {
        Ok(Some(requirement)) => requirement,
        Ok(None) => return,
        Err(error) => {
            log::warn!("Cannot read the .NET requirement of UnrealBuildTool: {error:#}");
            return;
        }
    };
    let Some(runtime) = bundled_runtime(engine_root, &requirement) else {
        return;
    };
    log::info!(
        "Using bundled .NET {} from {}",
        runtime.version,
        runtime.root.display()
    );
    for (name, value) in bundled_environment(&runtime.root, std::env::var_os("PATH")) {
        command.env(name, value);
    }
    for name in platform::DOTNET_ARCHITECTURE_ROOT_VARIABLES {
        command.env_remove(name);
    }
}

pub fn bundled_environment(
    root: &Path,
    current_path: Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    let search_path = std::iter::once(root.to_path_buf())
        .chain(
            current_path
                .iter()
                .flat_map(|path| std::env::split_paths(path).collect::<Vec<_>>()),
        )
        .collect::<Vec<_>>();
    let joined_path = std::env::join_paths(search_path).unwrap_or_else(|error| {
        log::warn!("Cannot extend PATH with the bundled .NET: {error}");
        current_path.clone().unwrap_or_default()
    });
    vec![
        ("DOTNET_ROOT", root.as_os_str().to_owned()),
        ("DOTNET_MULTILEVEL_LOOKUP", OsString::from("0")),
        ("DOTNET_ROLL_FORWARD", OsString::from("LatestMajor")),
        ("PATH", joined_path),
    ]
}

pub fn runtime_installer(channel: &str) -> Option<(String, &'static str)> {
    platform::DOTNET_RUNTIME_INSTALLER
        .map(|file| (format!("{DOWNLOAD_BASE_URL}/{channel}/{file}"), file))
}

pub fn runtime_installer_arguments(log_path: &Path) -> Vec<String> {
    vec![
        "/install".to_string(),
        "/passive".to_string(),
        "/norestart".to_string(),
        "/log".to_string(),
        log_path.display().to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const UE_5_8_RUNTIME_CONFIG: &str =
        include_str!("../tests/fixtures/UnrealBuildTool_5.8.runtimeconfig.json");
    const UE_5_6_RUNTIME_CONFIG: &str =
        include_str!("../tests/fixtures/UnrealBuildTool_5.6.runtimeconfig.json");

    fn version(text: &str) -> ToolVersion {
        ToolVersion::parse(text).unwrap()
    }

    #[test]
    fn parses_ue_5_8_runtime_config() {
        let requirement = parse_runtime_config(UE_5_8_RUNTIME_CONFIG)
            .unwrap()
            .unwrap();
        assert_eq!(requirement.version, version("10.0.0"));
        assert_eq!(requirement.roll_forward, RollForward::LatestMajor);
        assert_eq!(requirement.channel(), "10.0");
        assert!(!requirement.accepts(version("8.0.31")));
        assert!(requirement.accepts(version("10.0.7")));
        assert!(requirement.accepts(version("11.0.0")));
    }

    #[test]
    fn parses_ue_5_6_runtime_config_with_default_roll_forward() {
        let requirement = parse_runtime_config(UE_5_6_RUNTIME_CONFIG)
            .unwrap()
            .unwrap();
        assert_eq!(requirement.version, version("8.0.0"));
        assert_eq!(requirement.roll_forward, RollForward::Minor);
        assert!(requirement.accepts(version("8.0.31")));
        assert!(requirement.accepts(version("8.1.0")));
        assert!(!requirement.accepts(version("10.0.7")));
        assert!(!requirement.accepts(version("6.0.36")));
    }

    #[test]
    fn reads_framework_lists_and_ignores_other_frameworks() {
        let text = r#"{"runtimeOptions": {"rollForward": "LatestPatch", "frameworks": [
            {"name": "Microsoft.AspNetCore.App", "version": "9.0.0"},
            {"name": "Microsoft.NETCore.App", "version": "9.0.2"}
        ]}}"#;
        let requirement = parse_runtime_config(text).unwrap().unwrap();
        assert_eq!(requirement.version, version("9.0.2"));
        assert!(requirement.accepts(version("9.0.5")));
        assert!(!requirement.accepts(version("9.1.0")));
        assert_eq!(
            parse_runtime_config(r#"{"runtimeOptions": {}}"#).unwrap(),
            None
        );
        assert!(parse_runtime_config("not json").is_err());
    }

    #[test]
    fn picks_the_newest_accepted_runtime() {
        let requirement = parse_runtime_config(UE_5_8_RUNTIME_CONFIG)
            .unwrap()
            .unwrap();
        let installed = [version("6.0.36"), version("8.0.31")];
        assert_eq!(best_runtime(&requirement, &installed), None);
        let installed = [version("8.0.31"), version("10.0.2"), version("10.0.7")];
        assert_eq!(
            best_runtime(&requirement, &installed),
            Some(version("10.0.7"))
        );
    }

    fn fake_dotnet(root: &Path, runtimes: &[&str]) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(platform::DOTNET_EXECUTABLE), "").unwrap();
        for runtime in runtimes {
            std::fs::create_dir_all(root.join("shared").join(RUNTIME_FRAMEWORK).join(runtime))
                .unwrap();
        }
    }

    #[test]
    fn finds_the_bundled_runtime_of_an_engine() {
        let engine = tempfile::tempdir().unwrap();
        let bundle = engine
            .path()
            .join("Engine")
            .join("Binaries")
            .join("ThirdParty")
            .join("DotNet");
        let architecture = platform::DOTNET_BUNDLE_ARCHITECTURES.first().unwrap();
        let newest = bundle.join("10.0").join(architecture);
        fake_dotnet(&newest, &["10.0.7"]);
        fake_dotnet(&bundle.join("8.0.300").join(architecture), &["8.0.6"]);
        std::fs::create_dir_all(bundle.join("NotARuntime")).unwrap();
        assert_eq!(bundled_roots(engine.path()).len(), 2);
        let requirement = parse_runtime_config(UE_5_8_RUNTIME_CONFIG)
            .unwrap()
            .unwrap();
        assert_eq!(
            bundled_runtime(engine.path(), &requirement),
            Some(BundledRuntime {
                root: newest,
                version: version("10.0.7"),
            })
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(bundled_runtime(empty.path(), &requirement), None);
    }

    #[test]
    fn builds_the_bundled_environment() {
        let root = Path::new(r"C:\Program Files\Epic Games\UE_5.8\DotNet");
        let environment = bundled_environment(root, Some(OsString::from("existing")));
        let lookup = |name: &str| {
            environment
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(lookup("DOTNET_ROOT"), Some(root.as_os_str().to_owned()));
        assert_eq!(lookup("DOTNET_ROLL_FORWARD"), Some("LatestMajor".into()));
        let path = lookup("PATH").unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(entries, [root.to_path_buf(), PathBuf::from("existing")]);
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn builds_runtime_installer_url() {
        assert_eq!(
            runtime_installer("10.0"),
            Some((
                "https://aka.ms/dotnet/10.0/dotnet-runtime-win-x64.exe".to_string(),
                "dotnet-runtime-win-x64.exe"
            ))
        );
    }

    #[test]
    fn builds_runtime_installer_arguments() {
        let log = Path::new(r"C:\Temp\Unreal Hub\dotnet.log");
        assert_eq!(
            crate::toolchain::join_windows_arguments(&runtime_installer_arguments(log)),
            r#"/install /passive /norestart /log "C:\Temp\Unreal Hub\dotnet.log""#
        );
    }
}
