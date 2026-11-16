use crate::platform;
use crate::updates;
use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub const BUILD_TOOLS_DOWNLOAD_PAGE: &str = "https://visualstudio.microsoft.com/downloads/";
const BUILD_TOOLS_BOOTSTRAPPER_URL: &str = "https://aka.ms/vs/17/release/vs_buildtools.exe";
const BUILD_TOOLS_BOOTSTRAPPER_NAME: &str = "vs_buildtools.exe";
const BUILD_TOOLS_WORKLOAD: &str = "Microsoft.VisualStudio.Workload.VCTools";
const MSVC_COMPONENT: &str = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64";
const SDK_COMPONENT_PREFIX: &str = "Microsoft.VisualStudio.Component.Windows";
const DEFAULT_SDK_COMPONENT: &str = "Microsoft.VisualStudio.Component.Windows11SDK.22621";
const FIRST_WINDOWS_11_SDK_BUILD: u32 = 22000;
const VISUAL_STUDIO_2022_MAJOR: u32 = 17;
const VISUAL_STUDIO_2026_MAJOR: u32 = 18;

#[derive(Clone, Copy, Debug, Default)]
pub struct ToolVersion {
    parts: [u32; 4],
    length: usize,
}

impl ToolVersion {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = [0u32; 4];
        let mut length = 0;
        for piece in text.trim().split('.') {
            let slot = parts.get_mut(length)?;
            *slot = piece.trim().parse().ok()?;
            length += 1;
        }
        (length > 0).then_some(Self { parts, length })
    }

    pub fn major(&self) -> u32 {
        let [major, ..] = self.parts;
        major
    }

    fn build(&self) -> u32 {
        let [_, _, build, _] = self.parts;
        build
    }
}

impl PartialEq for ToolVersion {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
    }
}

impl Eq for ToolVersion {}

impl PartialOrd for ToolVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ToolVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parts.cmp(&other.parts)
    }
}

impl fmt::Display for ToolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self
            .parts
            .iter()
            .take(self.length)
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".");
        formatter.write_str(&text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRange {
    low: ToolVersion,
    high: ToolVersion,
}

impl VersionRange {
    pub fn parse(text: &str) -> Option<Self> {
        let (low, high) = text.split_once('-')?;
        Some(Self {
            low: ToolVersion::parse(low)?,
            high: ToolVersion::parse(high)?,
        })
    }

    pub fn contains(&self, version: ToolVersion) -> bool {
        self.low <= version && version <= self.high
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainRequirements {
    pub minimum_msvc: Option<ToolVersion>,
    pub banned_msvc: Vec<VersionRange>,
    pub minimum_sdk: Option<ToolVersion>,
    pub maximum_sdk: Option<ToolVersion>,
    pub sdk_component: String,
    pub minimum_visual_studio_2022: Option<ToolVersion>,
    pub minimum_visual_studio_2026: Option<ToolVersion>,
}

impl Default for ToolchainRequirements {
    fn default() -> Self {
        Self {
            minimum_msvc: None,
            banned_msvc: Vec::new(),
            minimum_sdk: None,
            maximum_sdk: None,
            sdk_component: DEFAULT_SDK_COMPONENT.to_string(),
            minimum_visual_studio_2022: None,
            minimum_visual_studio_2026: None,
        }
    }
}

impl ToolchainRequirements {
    pub fn supports_instance(&self, version: ToolVersion) -> bool {
        match version.major() {
            VISUAL_STUDIO_2022_MAJOR => self
                .minimum_visual_studio_2022
                .is_none_or(|minimum| version >= minimum),
            VISUAL_STUDIO_2026_MAJOR => self
                .minimum_visual_studio_2026
                .is_some_and(|minimum| version >= minimum),
            _ => false,
        }
    }

    pub fn allows_msvc(&self, version: ToolVersion) -> bool {
        self.minimum_msvc.is_none_or(|minimum| version >= minimum)
            && !self.banned_msvc.iter().any(|range| range.contains(version))
    }

    pub fn allows_sdk(&self, version: ToolVersion) -> bool {
        self.minimum_sdk.is_none_or(|minimum| version >= minimum)
            && self.maximum_sdk.is_none_or(|maximum| version <= maximum)
    }
}

pub fn windows_sdk_json_path(engine_root: &Path) -> PathBuf {
    engine_root
        .join("Engine")
        .join("Config")
        .join("Windows")
        .join("Windows_SDK.json")
}

pub fn parse_requirements(text: &str) -> Result<ToolchainRequirements> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    struct WindowsSdkFile {
        main_version: Option<String>,
        min_version: Option<String>,
        max_version: Option<String>,
        minimum_visual_cpp_version: Option<String>,
        banned_visual_cpp_versions: Vec<String>,
        visual_studio_suggested_components: Vec<String>,
        minimum_visual_studio2022_version: Option<String>,
        minimum_visual_studio2026_version: Option<String>,
    }

    let file: WindowsSdkFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .context("Windows_SDK.json is not valid JSON")?;
    let parse = |value: &Option<String>| value.as_deref().and_then(ToolVersion::parse);
    let sdk_component = file
        .visual_studio_suggested_components
        .iter()
        .find(|component| component.starts_with(SDK_COMPONENT_PREFIX) && component.contains("SDK."))
        .cloned()
        .or_else(|| parse(&file.main_version).map(sdk_component_for_version))
        .unwrap_or_else(|| DEFAULT_SDK_COMPONENT.to_string());
    Ok(ToolchainRequirements {
        minimum_msvc: parse(&file.minimum_visual_cpp_version),
        banned_msvc: file
            .banned_visual_cpp_versions
            .iter()
            .filter_map(|range| VersionRange::parse(range))
            .collect(),
        minimum_sdk: parse(&file.min_version),
        maximum_sdk: parse(&file.max_version),
        sdk_component,
        minimum_visual_studio_2022: parse(&file.minimum_visual_studio2022_version),
        minimum_visual_studio_2026: parse(&file.minimum_visual_studio2026_version),
    })
}

fn sdk_component_for_version(version: ToolVersion) -> String {
    let build = version.build();
    let family = if build >= FIRST_WINDOWS_11_SDK_BUILD {
        "Windows11SDK"
    } else {
        "Windows10SDK"
    };
    format!("Microsoft.VisualStudio.Component.{family}.{build}")
}

pub fn read_requirements(engine_root: &Path) -> Result<ToolchainRequirements> {
    let path = windows_sdk_json_path(engine_root);
    if !path.is_file() {
        return Ok(ToolchainRequirements::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    parse_requirements(&text).with_context(|| format!("Cannot parse {}", path.display()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualStudioInstance {
    pub install_path: PathBuf,
    pub version: ToolVersion,
    pub display_name: String,
    pub msvc_versions: Vec<ToolVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceSummary {
    pub install_path: PathBuf,
    pub version: ToolVersion,
    pub display_name: String,
}

pub fn parse_vswhere_instances(json: &str) -> Result<Vec<InstanceSummary>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct VswhereEntry {
        installation_path: String,
        installation_version: String,
        #[serde(default)]
        display_name: String,
    }

    let entries: Vec<VswhereEntry> =
        serde_json::from_str(json.trim_start_matches('\u{feff}').trim())
            .context("vswhere output is not valid JSON")?;
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            Some(InstanceSummary {
                install_path: PathBuf::from(entry.installation_path),
                version: ToolVersion::parse(&entry.installation_version)?,
                display_name: entry.display_name,
            })
        })
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentState {
    Found(ToolVersion),
    Missing,
    NotRequired,
}

impl ComponentState {
    pub fn is_satisfied(self) -> bool {
        !matches!(self, ComponentState::Missing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainStatus {
    pub compiler: ComponentState,
    pub windows_sdk: ComponentState,
    pub instances: Vec<VisualStudioInstance>,
}

impl ToolchainStatus {
    pub fn not_required() -> Self {
        Self {
            compiler: ComponentState::NotRequired,
            windows_sdk: ComponentState::NotRequired,
            instances: Vec::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.compiler.is_satisfied() && self.windows_sdk.is_satisfied()
    }
}

pub fn evaluate(
    requirements: &ToolchainRequirements,
    instances: Vec<VisualStudioInstance>,
    sdk_versions: &[ToolVersion],
) -> ToolchainStatus {
    let compiler = instances
        .iter()
        .filter(|instance| requirements.supports_instance(instance.version))
        .flat_map(|instance| instance.msvc_versions.iter().copied())
        .filter(|version| requirements.allows_msvc(*version))
        .max();
    let windows_sdk = sdk_versions
        .iter()
        .copied()
        .filter(|version| requirements.allows_sdk(*version))
        .max();
    ToolchainStatus {
        compiler: compiler.map_or(ComponentState::Missing, ComponentState::Found),
        windows_sdk: windows_sdk.map_or(ComponentState::Missing, ComponentState::Found),
        instances,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallPlan {
    Fresh {
        components: Vec<String>,
    },
    Modify {
        install_path: PathBuf,
        display_name: String,
        components: Vec<String>,
        update_first: bool,
    },
}

pub fn plan_install(requirements: &ToolchainRequirements, status: &ToolchainStatus) -> InstallPlan {
    let existing = status
        .instances
        .iter()
        .filter(|instance| instance.version.major() == VISUAL_STUDIO_2022_MAJOR)
        .max_by_key(|instance| instance.version);
    let Some(instance) = existing else {
        return InstallPlan::Fresh {
            components: vec![
                BUILD_TOOLS_WORKLOAD.to_string(),
                MSVC_COMPONENT.to_string(),
                requirements.sdk_component.clone(),
            ],
        };
    };
    let outdated = !requirements.supports_instance(instance.version);
    let only_unusable_compilers = !instance.msvc_versions.is_empty()
        && !instance
            .msvc_versions
            .iter()
            .any(|version| requirements.allows_msvc(*version));
    InstallPlan::Modify {
        install_path: instance.install_path.clone(),
        display_name: instance.display_name.clone(),
        components: vec![
            MSVC_COMPONENT.to_string(),
            requirements.sdk_component.clone(),
        ],
        update_first: outdated || only_unusable_compilers,
    }
}

pub fn bootstrapper_arguments(components: &[String]) -> Vec<String> {
    let mut arguments: Vec<String> = ["--passive", "--wait", "--norestart"]
        .into_iter()
        .map(String::from)
        .collect();
    for component in components {
        arguments.push("--add".to_string());
        arguments.push(component.clone());
    }
    arguments
}

pub fn update_arguments(install_path: &Path) -> Vec<String> {
    vec![
        "update".to_string(),
        "--installPath".to_string(),
        install_path.display().to_string(),
        "--passive".to_string(),
        "--norestart".to_string(),
    ]
}

pub fn modify_arguments(install_path: &Path, components: &[String]) -> Vec<String> {
    let mut arguments = vec![
        "modify".to_string(),
        "--installPath".to_string(),
        install_path.display().to_string(),
    ];
    for component in components {
        arguments.push("--add".to_string());
        arguments.push(component.clone());
    }
    arguments.push("--passive".to_string());
    arguments.push("--norestart".to_string());
    arguments
}

pub fn quote_windows_argument(argument: &str) -> String {
    let needs_quotes = argument.is_empty()
        || argument
            .chars()
            .any(|character| matches!(character, ' ' | '\t' | '\n' | '\u{b}' | '"'));
    if !needs_quotes {
        return argument.to_string();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0usize;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            other => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(other);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

pub fn join_windows_arguments(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| quote_windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Completed,
    RebootRequired,
    Cancelled,
    Failed(i32),
}

pub fn classify_exit_code(code: i32) -> InstallOutcome {
    match code {
        0 => InstallOutcome::Completed,
        3010 | 1641 => InstallOutcome::RebootRequired,
        1602 | 5004 | -1073741510 => InstallOutcome::Cancelled,
        other => InstallOutcome::Failed(other),
    }
}

pub fn failure_message(code: i32) -> String {
    let reason = match code {
        740 => tr!(
            "관리자 권한이 필요합니다.",
            "Administrator rights are required."
        ),
        1001 | 1618 => tr!(
            "다른 설치가 진행 중입니다. 끝난 뒤 다시 시도하세요.",
            "Another installation is running. Try again after it finishes."
        ),
        1003 | 8006 => tr!(
            "Visual Studio가 실행 중입니다. 닫은 뒤 다시 시도하세요.",
            "Visual Studio is running. Close it and try again."
        ),
        5003 | -1073720687 => tr!(
            "설치 파일을 내려받지 못했습니다. 인터넷 연결을 확인하세요.",
            "Could not download the installer. Check your internet connection."
        ),
        5007 | 8010 => tr!(
            "이 PC가 설치 요구 사항을 충족하지 않습니다.",
            "This PC does not meet the installation requirements."
        ),
        _ => tr!(
            "%TEMP% 폴더의 dd_로 시작하는 로그를 확인하세요.",
            "Check the logs starting with dd_ in the %TEMP% folder."
        ),
    };
    trf!(
        "설치에 실패했습니다 (코드 {code}). {reason}",
        "Installation failed (code {code}). {reason}"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallProgress {
    Downloading { received: u64, total: Option<u64> },
    Verifying,
    Updating,
    Installing,
}

pub fn installer_directory() -> PathBuf {
    std::env::temp_dir().join("UnrealHub").join("buildtools")
}

pub fn install(
    plan: &InstallPlan,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(InstallProgress),
) -> Result<InstallOutcome> {
    let working_directory = installer_directory();
    std::fs::create_dir_all(&working_directory).with_context(|| {
        trf!(
            "{} 폴더를 만들 수 없습니다",
            "Cannot create folder {}",
            working_directory.display()
        )
    })?;
    match plan {
        InstallPlan::Fresh { components } => {
            let bootstrapper = working_directory.join(BUILD_TOOLS_BOOTSTRAPPER_NAME);
            progress(InstallProgress::Downloading {
                received: 0,
                total: None,
            });
            updates::download_file(
                BUILD_TOOLS_BOOTSTRAPPER_URL,
                &bootstrapper,
                cancel,
                &mut |received, total| progress(InstallProgress::Downloading { received, total }),
            )?;
            progress(InstallProgress::Verifying);
            platform::verify_microsoft_signature(&bootstrapper)?;
            progress(InstallProgress::Installing);
            run_installer(
                &bootstrapper,
                &bootstrapper_arguments(components),
                &working_directory,
            )
        }
        InstallPlan::Modify {
            install_path,
            components,
            update_first,
            ..
        } => {
            let setup = platform::visual_studio_setup().ok_or_else(|| {
                tr_anyhow!(
                    "Visual Studio Installer를 찾을 수 없습니다",
                    "Visual Studio Installer not found"
                )
            })?;
            progress(InstallProgress::Verifying);
            platform::verify_microsoft_signature(&setup)?;
            if *update_first {
                progress(InstallProgress::Updating);
                let outcome =
                    run_installer(&setup, &update_arguments(install_path), &working_directory)?;
                if !matches!(
                    outcome,
                    InstallOutcome::Completed | InstallOutcome::RebootRequired
                ) {
                    return Ok(outcome);
                }
            }
            progress(InstallProgress::Installing);
            run_installer(
                &setup,
                &modify_arguments(install_path, components),
                &working_directory,
            )
        }
    }
}

fn run_installer(
    executable: &Path,
    arguments: &[String],
    working_directory: &Path,
) -> Result<InstallOutcome> {
    let parameters = join_windows_arguments(arguments);
    log::info!("Running {} {parameters}", executable.display());
    match platform::run_elevated(executable, &parameters, working_directory)? {
        platform::ElevatedRun::Declined => Ok(InstallOutcome::Cancelled),
        platform::ElevatedRun::Exited(code) => {
            log::info!("{} exited with {code}", executable.display());
            Ok(classify_exit_code(code))
        }
    }
}

pub fn inspect(engine_root: &Path) -> (ToolchainRequirements, ToolchainStatus) {
    let requirements = match read_requirements(engine_root) {
        Ok(requirements) => requirements,
        Err(error) => {
            log::warn!("Using default toolchain requirements: {error:#}");
            ToolchainRequirements::default()
        }
    };
    let status = platform::inspect_toolchain(&requirements);
    (requirements, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UE_5_8_SDK: &str = include_str!("../tests/fixtures/Windows_SDK_5.8.json");
    const UE_5_6_SDK: &str = include_str!("../tests/fixtures/Windows_SDK_5.6.json");

    fn version(text: &str) -> ToolVersion {
        ToolVersion::parse(text).unwrap()
    }

    fn instance(version_text: &str, msvc: &[&str]) -> VisualStudioInstance {
        VisualStudioInstance {
            install_path: PathBuf::from(format!(r"C:\VS\{version_text}")),
            version: version(version_text),
            display_name: "Visual Studio Build Tools 2022".into(),
            msvc_versions: msvc.iter().map(|text| version(text)).collect(),
        }
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(version("14.44.35211") > version("14.44.35207"));
        assert!(version("14.100") > version("14.99"));
        assert_eq!(version("10.0"), version("10.0.0.0"));
        assert_eq!(version("17.14.36015.10").to_string(), "17.14.36015.10");
        assert!(ToolVersion::parse("1.2.3.4.5").is_none());
        assert!(ToolVersion::parse("abc").is_none());
    }

    #[test]
    fn parses_ue_5_8_requirements() {
        let requirements = parse_requirements(UE_5_8_SDK).unwrap();
        assert_eq!(requirements.minimum_msvc, Some(version("14.38.33130")));
        assert_eq!(requirements.minimum_sdk, Some(version("10.0.19041.0")));
        assert_eq!(requirements.maximum_sdk, Some(version("10.9.99999.0")));
        assert_eq!(
            requirements.sdk_component,
            "Microsoft.VisualStudio.Component.Windows11SDK.22621"
        );
        assert_eq!(requirements.banned_msvc.len(), 4);
        assert_eq!(
            requirements.minimum_visual_studio_2026,
            Some(version("18.0"))
        );
        assert!(!requirements.allows_msvc(version("14.44.35207")));
        assert!(requirements.allows_msvc(version("14.44.35211")));
        assert!(!requirements.allows_msvc(version("14.41.34120")));
        assert!(!requirements.allows_msvc(version("14.37.0")));
        assert!(requirements.supports_instance(version("18.0.11111.0")));
    }

    #[test]
    fn ue_5_6_does_not_support_visual_studio_2026() {
        let requirements = parse_requirements(UE_5_6_SDK).unwrap();
        assert_eq!(requirements.minimum_visual_studio_2026, None);
        assert!(!requirements.supports_instance(version("18.0.11111.0")));
        assert!(requirements.supports_instance(version("17.8.0")));
        assert!(!requirements.supports_instance(version("17.7.5")));
        assert!(requirements.allows_msvc(version("14.44.35207")));
    }

    #[test]
    fn derives_sdk_component_from_main_version() {
        let requirements = parse_requirements(r#"{"MainVersion": "10.0.19041.0"}"#).unwrap();
        assert_eq!(
            requirements.sdk_component,
            "Microsoft.VisualStudio.Component.Windows10SDK.19041"
        );
        let requirements = parse_requirements(r#"{"MainVersion": "10.0.26100.0"}"#).unwrap();
        assert_eq!(
            requirements.sdk_component,
            "Microsoft.VisualStudio.Component.Windows11SDK.26100"
        );
    }

    #[test]
    fn parses_vswhere_output() {
        let json = r#"[
            {"instanceId": "a", "installationPath": "C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools",
             "installationVersion": "17.14.36015.10", "displayName": "Visual Studio Build Tools 2022"},
            {"installationPath": "C:\\Broken", "installationVersion": "not a version"}
        ]"#;
        let instances = parse_vswhere_instances(json).unwrap();
        assert_eq!(instances.len(), 1);
        let first = instances.first().unwrap();
        assert_eq!(first.version, version("17.14.36015.10"));
        assert_eq!(first.display_name, "Visual Studio Build Tools 2022");
        assert!(parse_vswhere_instances("[]").unwrap().is_empty());
    }

    #[test]
    fn evaluates_missing_toolchain() {
        let requirements = parse_requirements(UE_5_8_SDK).unwrap();
        let status = evaluate(&requirements, Vec::new(), &[]);
        assert_eq!(status.compiler, ComponentState::Missing);
        assert_eq!(status.windows_sdk, ComponentState::Missing);
        assert!(!status.is_ready());
        assert_eq!(
            plan_install(&requirements, &status),
            InstallPlan::Fresh {
                components: vec![
                    "Microsoft.VisualStudio.Workload.VCTools".into(),
                    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64".into(),
                    "Microsoft.VisualStudio.Component.Windows11SDK.22621".into(),
                ]
            }
        );
    }

    #[test]
    fn evaluates_ready_toolchain() {
        let requirements = parse_requirements(UE_5_8_SDK).unwrap();
        let status = evaluate(
            &requirements,
            vec![instance("17.14.36015.10", &["14.40.33807", "14.44.35211"])],
            &[version("10.0.18362.0"), version("10.0.22621.0")],
        );
        assert_eq!(
            status.compiler,
            ComponentState::Found(version("14.44.35211"))
        );
        assert_eq!(
            status.windows_sdk,
            ComponentState::Found(version("10.0.22621.0"))
        );
        assert!(status.is_ready());
    }

    #[test]
    fn ignores_compilers_from_unsupported_instances() {
        let requirements = parse_requirements(UE_5_6_SDK).unwrap();
        let status = evaluate(
            &requirements,
            vec![instance("18.0.11111.0", &["14.50.35723"])],
            &[version("10.0.22621.0")],
        );
        assert_eq!(status.compiler, ComponentState::Missing);
        assert!(matches!(
            plan_install(&requirements, &status),
            InstallPlan::Fresh { .. }
        ));
    }

    #[test]
    fn plans_modify_for_existing_visual_studio_2022() {
        let requirements = parse_requirements(UE_5_8_SDK).unwrap();
        let without_cpp = evaluate(&requirements, vec![instance("17.14.36015.10", &[])], &[]);
        match plan_install(&requirements, &without_cpp) {
            InstallPlan::Modify {
                components,
                update_first,
                ..
            } => {
                assert_eq!(components.len(), 2);
                assert!(!update_first);
            }
            other => panic!("unexpected plan {other:?}"),
        }
        let banned_only = evaluate(
            &requirements,
            vec![instance("17.10.0", &["14.40.33807"])],
            &[],
        );
        assert!(matches!(
            plan_install(&requirements, &banned_only),
            InstallPlan::Modify {
                update_first: true,
                ..
            }
        ));
    }

    #[test]
    fn builds_installer_arguments() {
        let components = vec!["A".to_string(), "B".to_string()];
        assert_eq!(
            bootstrapper_arguments(&components),
            [
                "--passive",
                "--wait",
                "--norestart",
                "--add",
                "A",
                "--add",
                "B"
            ]
        );
        let path = Path::new(r"C:\Program Files\VS");
        assert_eq!(
            join_windows_arguments(&modify_arguments(path, &components)),
            r#"modify --installPath "C:\Program Files\VS" --add A --add B --passive --norestart"#
        );
        assert_eq!(
            update_arguments(path),
            [
                "update",
                "--installPath",
                r"C:\Program Files\VS",
                "--passive",
                "--norestart"
            ]
        );
    }

    #[test]
    fn quotes_windows_arguments() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument(""), "\"\"");
        assert_eq!(quote_windows_argument(r"C:\a b\"), r#""C:\a b\\""#);
        assert_eq!(quote_windows_argument(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_windows_argument(r"C:\한글 경로"), r#""C:\한글 경로""#);
    }

    #[test]
    fn classifies_installer_exit_codes() {
        assert_eq!(classify_exit_code(0), InstallOutcome::Completed);
        assert_eq!(classify_exit_code(3010), InstallOutcome::RebootRequired);
        assert_eq!(classify_exit_code(1602), InstallOutcome::Cancelled);
        assert_eq!(classify_exit_code(-1073741510), InstallOutcome::Cancelled);
        assert_eq!(classify_exit_code(5007), InstallOutcome::Failed(5007));
    }
}
