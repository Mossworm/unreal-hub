use crate::platform;
use crate::templates::{Replacement, Template, TemplateDefs, TemplateSetting};
use crate::unreal::EngineVersion;
use anyhow::{Context as _, Result, anyhow, bail};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const MAX_PROJECT_NAME_LENGTH: usize = 20;
const MAX_PROJECT_PATH_LENGTH: usize = 130;
const LINE_TERMINATOR: &str = "\r\n";
const PROJECT_FILES_LOG: &str = "UnrealHub-GenerateProjectFiles.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareClass {
    Desktop,
    Mobile,
}

impl HardwareClass {
    fn ini_value(self) -> &'static str {
        match self {
            HardwareClass::Desktop => "Desktop",
            HardwareClass::Mobile => "Mobile",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsPreset {
    Maximum,
    Scalable,
}

impl GraphicsPreset {
    fn ini_value(self) -> &'static str {
        match self {
            GraphicsPreset::Maximum => "Maximum",
            GraphicsPreset::Scalable => "Scalable",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreationRequest {
    pub engine_root: PathBuf,
    pub engine_identifier: String,
    pub engine_version: Option<EngineVersion>,
    pub template: Template,
    pub project_name: String,
    pub location: PathBuf,
    pub hardware: HardwareClass,
    pub graphics: GraphicsPreset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationStage {
    CopyingTemplate,
    WritingConfig,
    CopyingSharedContent,
    WritingProjectFile,
    GeneratingProjectFiles,
}

impl CreationStage {
    pub fn label(self) -> &'static str {
        match self {
            CreationStage::CopyingTemplate => "템플릿 복사 중…",
            CreationStage::WritingConfig => "프로젝트 설정 기록 중…",
            CreationStage::CopyingSharedContent => "공유 콘텐츠 복사 중…",
            CreationStage::WritingProjectFile => ".uproject 생성 중…",
            CreationStage::GeneratingProjectFiles => "프로젝트 파일 생성 중…",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedProject {
    pub project_file: PathBuf,
    pub project_files_error: Option<String>,
}

#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("프로젝트 생성이 취소되었습니다")
    }
}

impl std::error::Error for Cancelled {}

pub fn validate_project_name(name: &str) -> Result<(), String> {
    let Some(first) = name.chars().next() else {
        return Err("프로젝트 이름을 입력하세요.".into());
    };
    if !first.is_ascii_alphabetic() {
        return Err("프로젝트 이름은 영문자로 시작해야 합니다.".into());
    }
    if let Some(invalid) = name
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || *character == '_'))
    {
        return Err(format!(
            "프로젝트 이름에는 영문, 숫자, _만 쓸 수 있습니다. ('{invalid}' 사용 불가)"
        ));
    }
    if name.chars().count() > MAX_PROJECT_NAME_LENGTH {
        return Err(format!(
            "프로젝트 이름은 {MAX_PROJECT_NAME_LENGTH}자 이하여야 합니다."
        ));
    }
    Ok(())
}

pub fn validate_location(location: &Path, name: &str, engine_root: &Path) -> Result<(), String> {
    if location.as_os_str().is_empty() {
        return Err("위치를 선택하세요.".into());
    }
    if !location.is_dir() {
        return Err("존재하지 않는 폴더입니다.".into());
    }
    let destination = location.join(name);
    if destination.exists() {
        return Err(format!("'{}' 폴더가 이미 있습니다.", destination.display()));
    }
    let project_path_length = destination.join(name).to_string_lossy().chars().count();
    if project_path_length > MAX_PROJECT_PATH_LENGTH {
        return Err(format!(
            "프로젝트 경로가 너무 깁니다. ({MAX_PROJECT_PATH_LENGTH}자 이하)"
        ));
    }
    let normalized_location = crate::paths::path_key(location);
    let engine_folder = crate::paths::path_key(&engine_root.join("Engine"));
    if normalized_location == engine_folder
        || normalized_location.starts_with(&format!("{engine_folder}\\"))
    {
        return Err("엔진 폴더 안에는 프로젝트를 만들 수 없습니다.".into());
    }
    Ok(())
}

pub fn suggest_project_name(location: &Path, base: &str) -> String {
    if !location.join(base).exists() {
        return base.to_string();
    }
    (1..1000)
        .map(|index| format!("{base}{index}"))
        .find(|candidate| !location.join(candidate).exists())
        .unwrap_or_else(|| base.to_string())
}

pub fn create_project(
    request: &CreationRequest,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(CreationStage),
) -> Result<CreatedProject> {
    validate_project_name(&request.project_name).map_err(|message| anyhow!(message))?;
    validate_location(
        &request.location,
        &request.project_name,
        &request.engine_root,
    )
    .map_err(|message| anyhow!(message))?;
    let destination = request.location.join(&request.project_name);
    std::fs::create_dir_all(destination.join("Content"))
        .with_context(|| format!("{} 폴더를 만들 수 없습니다", destination.display()))?;
    let result = populate_project(request, &destination, cancel, report);
    if result.is_err()
        && let Err(cleanup_error) = std::fs::remove_dir_all(&destination)
    {
        log::error!(
            "Failed to clean up {}: {cleanup_error}",
            destination.display()
        );
    }
    result
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Cancelled.into())
    } else {
        Ok(())
    }
}

fn populate_project(
    request: &CreationRequest,
    destination: &Path,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(CreationStage),
) -> Result<CreatedProject> {
    let template = &request.template;
    let features = EngineFeatures::detect(&request.engine_root, request.engine_version);
    let mut defs = template.defs.clone();
    if let Some(version) = &features.build_settings_version {
        defs.replacements_in_files.push(Replacement {
            extensions: vec!["cs".into()],
            from: "BuildSettingsVersion.Latest".into(),
            to: version.clone(),
            case_sensitive: true,
        });
    }
    let defs = defs.fixed_up(&template.name, &request.project_name);

    report(CreationStage::CopyingTemplate);
    let copied = copy_template_files(&template.folder, destination, &defs, cancel)?;
    for file in &copied.needs_replacement {
        check_cancel(cancel)?;
        replace_in_file(file, &defs.replacements_in_files)?;
    }

    report(CreationStage::WritingConfig);
    let config_values =
        template_config_values(request, &template.defs, &features, &copied.class_renames);
    for value in &config_values {
        apply_config_value_to_file(&destination.join("Config"), value)?;
    }

    report(CreationStage::CopyingSharedContent);
    copy_shared_content(request, &template.defs, destination, cancel)?;

    report(CreationStage::WritingProjectFile);
    let project_file = destination.join(format!("{}.uproject", request.project_name));
    write_project_descriptor(
        &template.uproject,
        &project_file,
        &template.name,
        &request.project_name,
        &request.engine_identifier,
    )?;

    let mut project_files_error = None;
    if template.generates_code {
        check_cancel(cancel)?;
        report(CreationStage::GeneratingProjectFiles);
        match generate_project_files(&request.engine_root, &project_file, cancel) {
            Ok(()) => {}
            Err(error) if error.is::<Cancelled>() => return Err(error),
            Err(error) => {
                log::warn!("Generating project files failed: {error:#}");
                project_files_error = Some(format!("{error:#}"));
            }
        }
    }
    Ok(CreatedProject {
        project_file,
        project_files_error,
    })
}

struct CopiedFiles {
    needs_replacement: Vec<PathBuf>,
    class_renames: Vec<(String, String)>,
}

fn relative_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .with_context(|| format!("{}을(를) 읽을 수 없습니다", directory.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn split_relative(relative: &str) -> (&str, &str) {
    match relative.rsplit_once('/') {
        Some((directory, file)) => (directory, file),
        None => ("", relative),
    }
}

fn split_extension(file_name: &str) -> (&str, &str) {
    match file_name.rsplit_once('.') {
        Some((base, extension)) if !base.is_empty() => (base, extension),
        _ => (file_name, ""),
    }
}

pub fn destination_subpath(relative: &str, defs: &TemplateDefs) -> Option<String> {
    if defs
        .files_to_ignore
        .iter()
        .any(|ignored| ignored.eq_ignore_ascii_case(relative))
    {
        return None;
    }
    let lowercase = relative.to_ascii_lowercase();
    if defs
        .folders_to_ignore
        .iter()
        .any(|folder| lowercase.starts_with(&format!("{}/", folder.to_ascii_lowercase())))
    {
        return None;
    }
    let (directory, file_name) = split_relative(relative);
    let mut destination_directory = format!("{directory}/");
    for rename in &defs.folder_renames {
        let prefix = format!("{}/", rename.from);
        if lowercase.starts_with(&prefix.to_ascii_lowercase()) {
            let remainder = destination_directory
                .get(rename.from.len()..)
                .unwrap_or_default()
                .to_string();
            destination_directory = format!("{}{remainder}", rename.to);
        }
    }
    let (base, extension) = split_extension(file_name);
    let mut destination_base = base.to_string();
    for replacement in &defs.filename_replacements {
        if replacement.applies_to(extension) {
            destination_base = replace_text(
                &destination_base,
                &replacement.from,
                &replacement.to,
                replacement.case_sensitive,
            );
        }
    }
    let directory_part = destination_directory.trim_start_matches('/');
    let file_part = if extension.is_empty() {
        destination_base
    } else {
        format!("{destination_base}.{extension}")
    };
    Some(format!("{directory_part}{file_part}"))
}

fn copy_template_files(
    source: &Path,
    destination: &Path,
    defs: &TemplateDefs,
    cancel: &AtomicBool,
) -> Result<CopiedFiles> {
    let mut copied = CopiedFiles {
        needs_replacement: Vec::new(),
        class_renames: Vec::new(),
    };
    for relative in relative_files(source)? {
        check_cancel(cancel)?;
        let Some(destination_relative) = destination_subpath(&relative, defs) else {
            continue;
        };
        let source_file = source.join(&relative);
        let destination_file = destination.join(&destination_relative);
        if let Some(parent) = destination_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("{} 폴더를 만들 수 없습니다", parent.display()))?;
        }
        std::fs::copy(&source_file, &destination_file).with_context(|| {
            format!(
                "\"{}\"을(를) \"{}\"(으)로 복사하지 못했습니다",
                source_file.display(),
                destination_file.display()
            )
        })?;
        let (_, source_name) = split_relative(&relative);
        let (_, destination_name) = split_relative(&destination_relative);
        let (source_base, extension) = split_extension(source_name);
        let (destination_base, _) = split_extension(destination_name);
        if defs
            .replacements_in_files
            .iter()
            .any(|replacement| replacement.applies_to(extension))
        {
            copied.needs_replacement.push(destination_file.clone());
        }
        if source_base != destination_base
            && extension.eq_ignore_ascii_case("h")
            && std::fs::read_to_string(&destination_file)
                .is_ok_and(|contents| contents.to_ascii_lowercase().contains(".generated.h\""))
        {
            copied
                .class_renames
                .push((source_base.to_string(), destination_base.to_string()));
        }
    }
    Ok(copied)
}

pub fn replace_text(haystack: &str, from: &str, to: &str, case_sensitive: bool) -> String {
    if from.is_empty() {
        return haystack.to_string();
    }
    if case_sensitive {
        return haystack.replace(from, to);
    }
    let lower_haystack = haystack.to_ascii_lowercase();
    let lower_from = from.to_ascii_lowercase();
    let mut result = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(found) = lower_haystack
        .get(cursor..)
        .and_then(|rest| rest.find(&lower_from))
    {
        let start = cursor + found;
        result.push_str(haystack.get(cursor..start).unwrap_or_default());
        result.push_str(to);
        cursor = start + from.len();
    }
    result.push_str(haystack.get(cursor..).unwrap_or_default());
    result
}

pub fn apply_replacements(contents: &str, extension: &str, replacements: &[Replacement]) -> String {
    let placeholder = |index: usize| format!("{{{{{{REPLACE:{index}}}}}}}");
    let mut text = contents.to_string();
    for (index, replacement) in replacements.iter().enumerate() {
        if replacement.applies_to(extension) {
            text = replace_text(
                &text,
                &replacement.from,
                &placeholder(index),
                replacement.case_sensitive,
            );
        }
    }
    for (index, replacement) in replacements.iter().enumerate() {
        if replacement.applies_to(extension) {
            text = text.replace(&placeholder(index), &replacement.to);
        }
    }
    text
}

fn replace_in_file(path: &Path, replacements: &[Replacement]) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("{}을(를) 읽을 수 없습니다", path.display()))?;
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(body) => (true, body),
        None => (false, bytes.as_slice()),
    };
    let Ok(text) = std::str::from_utf8(body) else {
        log::warn!("Skipping replacements in non-UTF-8 file {}", path.display());
        return Ok(());
    };
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
        .unwrap_or_default();
    let replaced = apply_replacements(text, &extension, replacements);
    if replaced == text {
        return Ok(());
    }
    let mut output = Vec::with_capacity(replaced.len() + 3);
    if bom {
        output.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    output.extend_from_slice(replaced.as_bytes());
    std::fs::write(path, output)
        .with_context(|| format!("{}을(를) 처리하지 못했습니다", path.display()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineFeatures {
    pub substrate_defaults: bool,
    pub common_ui_console_variables: bool,
    pub build_settings_version: Option<String>,
}

impl EngineFeatures {
    fn detect(engine_root: &Path, version: Option<EngineVersion>) -> Self {
        let source = engine_root
            .join("Engine")
            .join("Source")
            .join("Editor")
            .join("GameProjectGeneration")
            .join("Private")
            .join("GameProjectUtils.cpp");
        match std::fs::read_to_string(&source) {
            Ok(text) => Self::from_source(&text),
            Err(_) => {
                let modern =
                    version.is_some_and(|version| (version.major, version.minor) >= (5, 8));
                Self {
                    substrate_defaults: modern,
                    common_ui_console_variables: modern,
                    build_settings_version: None,
                }
            }
        }
    }

    pub fn from_source(text: &str) -> Self {
        Self {
            substrate_defaults: text
                .contains("AddSubstrateConfigValues(InProjectInfo, ConfigValuesToSet)"),
            common_ui_console_variables: text.contains("CommonUI.CheckKeyboardFocusAndParentage"),
            build_settings_version: text
                .find("GameProjectUtils::GetDefaultBuildSettingsVersion()")
                .and_then(|start| text.get(start..))
                .and_then(|rest| rest.find("TEXT(\"").and_then(|index| rest.get(index + 6..)))
                .and_then(|rest| rest.find("\")").and_then(|end| rest.get(..end)))
                .filter(|version| version.starts_with("BuildSettingsVersion."))
                .map(str::to_string),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigValue {
    pub file: &'static str,
    pub section: String,
    pub key: String,
    pub value: String,
    pub replace_existing: bool,
}

fn config_value(
    file: &'static str,
    section: &str,
    key: &str,
    value: &str,
    replace_existing: bool,
) -> ConfigValue {
    ConfigValue {
        file,
        section: section.to_string(),
        key: key.to_string(),
        value: value.to_string(),
        replace_existing,
    }
}

const ENGINE_INI: &str = "DefaultEngine.ini";
const GAME_INI: &str = "DefaultGame.ini";
const RENDERER: &str = "/Script/Engine.RendererSettings";

fn template_config_values(
    request: &CreationRequest,
    defs: &TemplateDefs,
    features: &EngineFeatures,
    class_renames: &[(String, String)],
) -> Vec<ConfigValue> {
    let hardware_section = "/Script/HardwareTargeting.HardwareTargetingSettings";
    let mut values = Vec::new();
    if !defs.hides(TemplateSetting::HardwareTarget) {
        values.push(config_value(
            ENGINE_INI,
            hardware_section,
            "TargetedHardwareClass",
            request.hardware.ini_value(),
            true,
        ));
    }
    if !defs.hides(TemplateSetting::GraphicsPreset) {
        values.push(config_value(
            ENGINE_INI,
            hardware_section,
            "DefaultGraphicsPerformance",
            request.graphics.ini_value(),
            true,
        ));
    }
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.GenerateMeshDistanceFields",
        "True",
        true,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.DynamicGlobalIlluminationMethod",
        "1",
        true,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.ReflectionMethod",
        "1",
        true,
    ));
    values.push(config_value(
        ENGINE_INI,
        "/Script/WindowsTargetPlatform.WindowsTargetSettings",
        "DefaultGraphicsRHI",
        "DefaultGraphicsRHI_DX12",
        false,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.SkinCache.CompileShaders",
        "True",
        false,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.RayTracing",
        "True",
        false,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.RayTracing.RayTracingProxies.ProjectEnabled",
        "True",
        false,
    ));
    if features.substrate_defaults {
        values.push(config_value(
            ENGINE_INI,
            RENDERER,
            "r.Substrate",
            "True",
            false,
        ));
        values.push(config_value(
            ENGINE_INI,
            RENDERER,
            "r.Substrate.ProjectGBufferFormat",
            "0",
            false,
        ));
    }
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.Shadow.Virtual.Enable",
        "1",
        true,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.DefaultFeature.AutoExposure.ExtendDefaultLuminanceRange",
        "True",
        false,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.DefaultFeature.LocalExposure.HighlightContrastScale",
        "0.8",
        false,
    ));
    values.push(config_value(
        ENGINE_INI,
        RENDERER,
        "r.DefaultFeature.LocalExposure.ShadowContrastScale",
        "0.8",
        false,
    ));
    if defs.is_blank {
        values.push(config_value(
            ENGINE_INI,
            "/Script/WorldPartitionEditor.WorldPartitionEditorSettings",
            "CommandletClass",
            "Class'/Script/UnrealEd.WorldPartitionConvertCommandlet'",
            true,
        ));
        let interface = "/Script/Engine.UserInterfaceSettings";
        values.push(config_value(
            ENGINE_INI,
            interface,
            "bAuthorizeAutomaticWidgetVariableCreation",
            "False",
            true,
        ));
        values.push(config_value(
            ENGINE_INI,
            interface,
            "FontDPIPreset",
            "Standard",
            true,
        ));
        values.push(config_value(ENGINE_INI, interface, "FontDPI", "72", true));
        values.push(config_value(
            GAME_INI,
            "/Script/CommonUI.CommonUISettings",
            "CommonButtonAcceptKeyHandling",
            "TriggerClick",
            true,
        ));
    }
    if features.common_ui_console_variables {
        for key in [
            "CommonUI.CheckKeyboardFocusAndParentage",
            "CommonUI.DisallowUserFocusedWidgetForPendingFocusRecipient",
            "CommonUI.FallbackToDesiredOnAutoRestoreFailure",
        ] {
            values.push(config_value(GAME_INI, "ConsoleVariables", key, "1", true));
        }
    }
    let template_name = &request.template.name;
    let project_name = &request.project_name;
    values.push(config_value(
        ENGINE_INI,
        "/Script/Engine.Engine",
        "+ActiveGameNameRedirects",
        &format!(
            "(OldGameName=\"/Script/{template_name}\",NewGameName=\"/Script/{project_name}\")"
        ),
        false,
    ));
    values.push(config_value(
        ENGINE_INI,
        "/Script/Engine.Engine",
        "+ActiveGameNameRedirects",
        &format!("(OldGameName=\"{template_name}\",NewGameName=\"/Script/{project_name}\")"),
        false,
    ));
    let project_id = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
    values.push(config_value(
        GAME_INI,
        "/Script/EngineSettings.GeneralProjectSettings",
        "ProjectID",
        &project_id,
        true,
    ));
    for (old_name, new_name) in class_renames {
        values.push(config_value(
            ENGINE_INI,
            "/Script/Engine.Engine",
            "+ActiveClassRedirects",
            &format!("(OldClassName=\"{old_name}\",NewClassName=\"{new_name}\")"),
            false,
        ));
    }
    values
}

fn split_ini_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'\r' || byte == b'\n' {
            lines.push(text.get(start..index).unwrap_or_default());
            index += if byte == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            start = index;
        } else {
            index += 1;
        }
    }
    lines.push(text.get(start..).unwrap_or_default());
    lines
}

pub fn apply_config_value(text: &str, value: &ConfigValue) -> String {
    let lines = split_ini_lines(text);
    let entry = format!("{}={}", value.key, value.value);
    let mut output = String::new();
    let mut current_section = String::new();
    let mut found = false;
    for (index, raw_line) in lines.iter().enumerate() {
        let line = raw_line.trim();
        let mut exclude = false;
        if !found {
            if line.is_empty() || line.starts_with(';') {
            } else if line.starts_with('[') {
                if current_section.eq_ignore_ascii_case(&value.section) {
                    output.push_str(&entry);
                    output.push_str(LINE_TERMINATOR);
                    output.push_str(LINE_TERMINATOR);
                    found = true;
                }
                current_section = line
                    .get(1..line.len().saturating_sub(1))
                    .unwrap_or_default()
                    .to_string();
            } else if current_section.eq_ignore_ascii_case(&value.section)
                && let Some((key, _)) = line.split_once('=')
                && key.eq_ignore_ascii_case(&value.key)
            {
                output.push_str(&entry);
                output.push_str(LINE_TERMINATOR);
                exclude = value.replace_existing;
                found = true;
            }
        }
        if !exclude {
            output.push_str(line);
            if index + 1 != lines.len() {
                output.push_str(LINE_TERMINATOR);
            }
        }
    }
    if !found {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push_str(LINE_TERMINATOR);
        }
        if !current_section.eq_ignore_ascii_case(&value.section) {
            output.push_str(LINE_TERMINATOR);
            output.push_str(&format!("[{}]", value.section));
            output.push_str(LINE_TERMINATOR);
        }
        output.push_str(&entry);
        output.push_str(LINE_TERMINATOR);
    }
    output
}

fn apply_config_value_to_file(config_dir: &Path, value: &ConfigValue) -> Result<()> {
    let path = config_dir.join(value.file);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let text = text.trim_start_matches('\u{feff}');
    std::fs::write(&path, apply_config_value(text, value))
        .with_context(|| format!("{}을(를) 처리하지 못했습니다", path.display()))
}

pub fn shared_content_destination(relative: &str, mount_name: &str) -> Option<String> {
    const FOLDER_IDENTS: [(&str, bool); 3] = [
        ("__ExternalActors__/", true),
        ("__ExternalObjects__/", true),
        ("Content/", false),
    ];
    for (ident, prepend_content) in FOLDER_IDENTS {
        if let Some(index) = relative.find(ident) {
            let mut content_file = relative.get(index..)?.to_string();
            if !mount_name.is_empty() {
                let game_folder = if mount_name.starts_with('/') {
                    mount_name.to_string()
                } else {
                    format!("/{mount_name}")
                };
                content_file.insert_str(ident.len() - 1, &game_folder);
            }
            if prepend_content {
                content_file.insert_str(0, "Content/");
            }
            return Some(content_file);
        }
    }
    None
}

pub fn matches_wildcard(name: &str, pattern: &str) -> bool {
    fn matches(name: &[char], pattern: &[char]) -> bool {
        match pattern.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => (0..=name.len())
                .any(|skip| name.get(skip..).is_some_and(|tail| matches(tail, rest))),
            Some(('?', rest)) => name
                .split_first()
                .is_some_and(|(_, tail)| matches(tail, rest)),
            Some((expected, rest)) => name.split_first().is_some_and(|(actual, tail)| {
                actual.eq_ignore_ascii_case(expected) && matches(tail, rest)
            }),
        }
    }
    let name: Vec<char> = name.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    matches(&name, &pattern)
}

fn expand_additional_files(engine_root: &Path, file_source: &str) -> Result<Vec<String>> {
    let normalized = file_source.replace('\\', "/");
    let (directory, file_pattern) = split_relative(&normalized);
    if !file_pattern.contains('*') && !file_pattern.contains('?') {
        return Ok(vec![normalized.clone()]);
    }
    let base = engine_root.join(directory);
    if !base.is_dir() {
        bail!("공유 콘텐츠 폴더를 찾을 수 없습니다: {}", base.display());
    }
    Ok(relative_files(&base)?
        .into_iter()
        .filter(|relative| matches_wildcard(split_relative(relative).1, file_pattern))
        .map(|relative| format!("{directory}/{relative}"))
        .collect())
}

fn copy_shared_content(
    request: &CreationRequest,
    defs: &TemplateDefs,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let required_level = if !defs.hides(TemplateSetting::HardwareTarget)
        && request.hardware == HardwareClass::Mobile
    {
        "Standard"
    } else {
        "High"
    };
    for pack in &defs.shared_content_packs {
        let level = pack
            .detail_levels
            .iter()
            .find(|level| level.eq_ignore_ascii_case(required_level))
            .or_else(|| pack.detail_levels.first())
            .ok_or_else(|| anyhow!("공유 콘텐츠 팩 {}에 상세 수준이 없습니다", pack.mount_name))?;
        let manifest_path = request
            .engine_root
            .join("Templates")
            .join("TemplateResources")
            .join(level)
            .join(&pack.mount_name)
            .join("FeaturePack")
            .join("manifest.json");
        let manifest_text = std::fs::read_to_string(&manifest_path).with_context(|| {
            format!(
                "공유 콘텐츠 팩 {}{}을(를) 찾을 수 없습니다",
                pack.mount_name, level
            )
        })?;
        let manifest: serde_json::Value =
            serde_json::from_str(manifest_text.trim_start_matches('\u{feff}'))
                .with_context(|| format!("{}을(를) 해석할 수 없습니다", manifest_path.display()))?;
        let sources: Vec<String> = manifest
            .get("AdditionalFiles")
            .and_then(|files| files.get("AdditionalFilesList"))
            .and_then(|list| list.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for source in sources {
            for relative in expand_additional_files(&request.engine_root, &source)? {
                check_cancel(cancel)?;
                let Some(destination_relative) =
                    shared_content_destination(&relative, &pack.mount_name)
                else {
                    continue;
                };
                let source_file = request.engine_root.join(&relative);
                let destination_file = destination.join(&destination_relative);
                if let Some(parent) = destination_file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&source_file, &destination_file).with_context(|| {
                    format!("{}을(를) 복사하지 못했습니다", source_file.display())
                })?;
            }
        }
    }
    Ok(())
}

pub fn build_project_descriptor(
    template_text: &str,
    template_name: &str,
    project_name: &str,
    engine_identifier: &str,
) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(template_text.trim_start_matches('\u{feff}'))
            .context("템플릿 .uproject를 해석할 수 없습니다")?;
    let serde_json::Value::Object(template_object) = value else {
        bail!("템플릿 .uproject 형식이 올바르지 않습니다");
    };
    let mut object = serde_json::Map::new();
    let mut association_written = false;
    for (key, mut field) in template_object {
        match key.as_str() {
            "EpicSampleNameHash" => continue,
            "EngineAssociation" => {
                field = serde_json::Value::String(engine_identifier.to_string());
                association_written = true;
            }
            "Modules" => {
                if let serde_json::Value::Array(modules) = &mut field {
                    for module in modules {
                        if let Some(serde_json::Value::String(name)) = module.get_mut("Name") {
                            *name = replace_text(name, template_name, project_name, false);
                        }
                    }
                }
            }
            _ => {}
        }
        let is_file_version = key == "FileVersion";
        object.insert(key, field);
        if is_file_version && !association_written {
            object.insert(
                "EngineAssociation".into(),
                serde_json::Value::String(engine_identifier.to_string()),
            );
            association_written = true;
        }
    }
    if !association_written {
        object.insert(
            "EngineAssociation".into(),
            serde_json::Value::String(engine_identifier.to_string()),
        );
    }
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    serde::Serialize::serialize(&serde_json::Value::Object(object), &mut serializer)?;
    let mut text = String::from_utf8(buffer)?;
    text.push('\n');
    Ok(text)
}

fn write_project_descriptor(
    template_uproject: &Path,
    project_file: &Path,
    template_name: &str,
    project_name: &str,
    engine_identifier: &str,
) -> Result<()> {
    let template_text = std::fs::read_to_string(template_uproject)
        .with_context(|| format!("{}을(를) 읽을 수 없습니다", template_uproject.display()))?;
    let descriptor = build_project_descriptor(
        &template_text,
        template_name,
        project_name,
        engine_identifier,
    )?;
    std::fs::write(project_file, descriptor)
        .with_context(|| format!("{}을(를) 쓸 수 없습니다", project_file.display()))
}

fn generate_project_files(
    engine_root: &Path,
    project_file: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let build_tool = platform::unreal_build_tool(engine_root)
        .ok_or_else(|| anyhow!("UnrealBuildTool을 찾을 수 없습니다"))?;
    let log_directory = project_file
        .parent()
        .map(|directory| directory.join("Saved").join("Logs"))
        .ok_or_else(|| anyhow!("프로젝트 폴더를 알 수 없습니다"))?;
    std::fs::create_dir_all(&log_directory)?;
    let log_path = log_directory.join(PROJECT_FILES_LOG);
    let log_file = std::fs::File::create(&log_path)?;
    let error_log = log_file.try_clone()?;
    let distribution_flag = if engine_root
        .join("Engine")
        .join("Build")
        .join("SourceDistribution.txt")
        .is_file()
    {
        "-engine"
    } else {
        "-rocket"
    };
    let mut command = std::process::Command::new(&build_tool);
    command
        .arg("-projectfiles")
        .arg(format!("-project={}", project_file.display()))
        .arg("-game")
        .arg(distribution_flag)
        .arg("-progress")
        .current_dir(engine_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(error_log));
    platform::configure_hidden(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("{} 실행에 실패했습니다", build_tool.display()))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            if let Err(error) = child.kill() {
                log::warn!("Cannot stop UnrealBuildTool: {error}");
            }
            if let Err(error) = child.wait() {
                log::warn!("Cannot reap UnrealBuildTool: {error}");
            }
            return Err(Cancelled.into());
        }
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let tail = std::fs::read_to_string(&log_path)
                .map(|text| {
                    text.lines()
                        .rev()
                        .filter(|line| !line.trim().is_empty())
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            bail!("UnrealBuildTool 종료 코드 {status}: {tail}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::parse_template_defs;

    const THIRD_PERSON_DEFS: &str = include_str!("../tests/fixtures/TemplateDefs_ThirdPerson.ini");

    #[test]
    fn validates_project_names() {
        assert!(validate_project_name("MyProject").is_ok());
        assert!(validate_project_name("My_Game2").is_ok());
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("1Game").is_err());
        assert!(validate_project_name("My Game").is_err());
        assert!(validate_project_name("게임").is_err());
        assert!(validate_project_name("A123456789012345678901").is_err());
        assert!(validate_project_name("A1234567890123456789").is_ok());
    }

    #[test]
    fn suggests_unique_names() {
        let location = tempfile::tempdir().unwrap();
        assert_eq!(
            suggest_project_name(location.path(), "MyProject"),
            "MyProject"
        );
        std::fs::create_dir(location.path().join("MyProject")).unwrap();
        std::fs::create_dir(location.path().join("MyProject1")).unwrap();
        assert_eq!(
            suggest_project_name(location.path(), "MyProject"),
            "MyProject2"
        );
    }

    #[test]
    fn rejects_existing_destination_and_engine_folder() {
        let location = tempfile::tempdir().unwrap();
        std::fs::create_dir(location.path().join("Taken")).unwrap();
        let engine = location.path().join("UE");
        std::fs::create_dir_all(engine.join("Engine").join("Projects")).unwrap();
        assert!(validate_location(location.path(), "Fresh", &engine).is_ok());
        assert!(validate_location(location.path(), "Taken", &engine).is_err());
        assert!(validate_location(&location.path().join("Missing"), "Fresh", &engine).is_err());
        assert!(
            validate_location(&engine.join("Engine").join("Projects"), "Fresh", &engine).is_err()
        );
    }

    #[test]
    fn maps_template_paths_like_the_editor() {
        let defs = parse_template_defs(THIRD_PERSON_DEFS).fixed_up("TP_ThirdPerson", "MyGame");
        let map = |path: &str| destination_subpath(path, &defs);
        assert_eq!(map("TP_ThirdPerson.uproject"), None);
        assert_eq!(map("Config/TemplateDefs.ini"), None);
        assert_eq!(map("Media/TP_ThirdPerson.png"), None);
        assert_eq!(map("Content/ThirdPerson/Animations/Run.uasset"), None);
        assert_eq!(
            map("Config/DefaultEngine.ini").as_deref(),
            Some("Config/DefaultEngine.ini")
        );
        assert_eq!(
            map("Source/TP_ThirdPerson/TP_ThirdPersonCharacter.cpp").as_deref(),
            Some("Source/MyGame/MyGameCharacter.cpp")
        );
        assert_eq!(
            map("Source/TP_ThirdPerson.Target.cs").as_deref(),
            Some("Source/MyGame.Target.cs")
        );
        assert_eq!(
            map("Source/TP_ThirdPersonEditor.Target.cs").as_deref(),
            Some("Source/MyGameEditor.Target.cs")
        );
        assert_eq!(
            map("Content/ThirdPerson/Blueprints/BP_Character.uasset").as_deref(),
            Some("Content/ThirdPerson/Blueprints/BP_Character.uasset")
        );
    }

    #[test]
    fn replaces_file_contents_in_two_passes() {
        let defs = parse_template_defs(THIRD_PERSON_DEFS).fixed_up("TP_ThirdPerson", "MyGame");
        let source =
            "#include \"TP_ThirdPerson.h\"\nclass TP_THIRDPERSON_API Atp_thirdpersonCharacter;\n";
        assert_eq!(
            apply_replacements(source, "h", &defs.replacements_in_files),
            "#include \"MyGame.h\"\nclass MYGAME_API AmygameCharacter;\n"
        );
        assert_eq!(
            apply_replacements(source, "uasset", &defs.replacements_in_files),
            source
        );
    }

    #[test]
    fn replaces_text_case_insensitively() {
        assert_eq!(
            replace_text("TP_Blank tp_blank Tp_BLANK", "tp_blank", "Game", false),
            "Game Game Game"
        );
        assert_eq!(
            replace_text("TP_Blank tp_blank", "tp_blank", "Game", true),
            "TP_Blank Game"
        );
    }

    #[test]
    fn writes_config_values_like_the_editor() {
        let text = "[/Script/Engine.RendererSettings]\r\nr.ReflectionMethod=0\r\n\r\n[/Script/Engine.Engine]\r\n+ActiveGameNameRedirects=(Old)\r\n";
        let replaced = apply_config_value(
            text,
            &config_value(ENGINE_INI, RENDERER, "r.ReflectionMethod", "1", true),
        );
        assert_eq!(
            replaced,
            "[/Script/Engine.RendererSettings]\r\nr.ReflectionMethod=1\r\n\r\n[/Script/Engine.Engine]\r\n+ActiveGameNameRedirects=(Old)\r\n"
        );
        let kept = apply_config_value(
            text,
            &config_value(ENGINE_INI, RENDERER, "r.ReflectionMethod", "1", false),
        );
        assert!(kept.contains("r.ReflectionMethod=1\r\nr.ReflectionMethod=0"));
        let appended_in_section = apply_config_value(
            text,
            &config_value(ENGINE_INI, RENDERER, "r.RayTracing", "True", false),
        );
        assert!(appended_in_section.starts_with(
            "[/Script/Engine.RendererSettings]\r\nr.ReflectionMethod=0\r\n\r\nr.RayTracing=True\r\n\r\n[/Script/Engine.Engine]"
        ));
        let new_section = apply_config_value(
            text,
            &config_value(GAME_INI, "ConsoleVariables", "CommonUI.X", "1", true),
        );
        assert!(new_section.ends_with("(Old)\r\n\r\n[ConsoleVariables]\r\nCommonUI.X=1\r\n"));
        let before_existing = apply_config_value(
            text,
            &config_value(
                ENGINE_INI,
                "/Script/Engine.Engine",
                "+ActiveGameNameRedirects",
                "(New)",
                false,
            ),
        );
        assert!(
            before_existing
                .ends_with("+ActiveGameNameRedirects=(New)\r\n+ActiveGameNameRedirects=(Old)\r\n")
        );
        let mut successive = apply_config_value(
            text,
            &config_value(ENGINE_INI, "/Script/Engine.Engine", "Second", "2", true),
        );
        successive = apply_config_value(
            &successive,
            &config_value(ENGINE_INI, "/Script/Engine.Engine", "Third", "3", true),
        );
        assert!(successive.ends_with("+ActiveGameNameRedirects=(Old)\r\nSecond=2\r\nThird=3\r\n"));
        let without_trailing_newline =
            apply_config_value("[A]\nx=1", &config_value(ENGINE_INI, "A", "y", "2", true));
        assert_eq!(without_trailing_newline, "[A]\r\nx=1\r\ny=2\r\n");
    }

    #[test]
    fn maps_shared_content_into_mount_folder() {
        assert_eq!(
            shared_content_destination(
                "Templates/TemplateResources/High/LevelPrototyping/Content/Materials/M_Grid.uasset",
                "LevelPrototyping"
            )
            .as_deref(),
            Some("Content/LevelPrototyping/Materials/M_Grid.uasset")
        );
        assert_eq!(
            shared_content_destination(
                "Templates/TemplateResources/High/Input/Content/__ExternalActors__/Map/A.uasset",
                "Input"
            )
            .as_deref(),
            Some("Content/__ExternalActors__/Input/Map/A.uasset")
        );
        assert_eq!(
            shared_content_destination("Templates/readme.txt", "Input"),
            None
        );
    }

    #[test]
    fn matches_unreal_wildcards() {
        assert!(matches_wildcard("M_Grid.uasset", "*.*"));
        assert!(matches_wildcard("M_Grid.uasset", "*.UASSET"));
        assert!(!matches_wildcard("README", "*.*"));
        assert!(matches_wildcard("README", "*"));
        assert!(matches_wildcard("a1.txt", "a?.txt"));
    }

    #[test]
    fn builds_project_descriptor_from_template() {
        let template = "{\n\t\"FileVersion\": 3,\n\t\"EngineAssociation\": \"\",\n\t\"EpicSampleNameHash\": \"123\",\n\t\"Modules\": [\n\t\t{\n\t\t\t\"Name\": \"TP_ThirdPerson\",\n\t\t\t\"Type\": \"Runtime\"\n\t\t}\n\t]\n}";
        let descriptor =
            build_project_descriptor(template, "TP_ThirdPerson", "MyGame", "5.8").unwrap();
        assert_eq!(
            descriptor,
            "{\n\t\"FileVersion\": 3,\n\t\"EngineAssociation\": \"5.8\",\n\t\"Modules\": [\n\t\t{\n\t\t\t\"Name\": \"MyGame\",\n\t\t\t\"Type\": \"Runtime\"\n\t\t}\n\t]\n}\n"
        );
        let without_association = build_project_descriptor(
            "{\"FileVersion\": 3, \"Category\": \"\"}",
            "T",
            "P",
            "{GUID}",
        )
        .unwrap();
        assert!(without_association.starts_with(
            "{\n\t\"FileVersion\": 3,\n\t\"EngineAssociation\": \"{GUID}\",\n\t\"Category\""
        ));
    }

    #[test]
    fn detects_engine_features_from_source() {
        let source = "const TCHAR* GameProjectUtils::GetDefaultBuildSettingsVersion()\n{\n\treturn TEXT(\"BuildSettingsVersion.V7\");\n}\nAddSubstrateConfigValues(InProjectInfo, ConfigValuesToSet);";
        let features = EngineFeatures::from_source(source);
        assert_eq!(
            features.build_settings_version.as_deref(),
            Some("BuildSettingsVersion.V7")
        );
        assert!(features.substrate_defaults);
        assert!(!features.common_ui_console_variables);
    }

    fn fixture_engine(root: &Path) -> Template {
        let template_dir = root.join("Templates").join("TP_Code");
        let source_dir = template_dir.join("Source").join("TP_Code");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(template_dir.join("Config")).unwrap();
        std::fs::create_dir_all(template_dir.join("Content").join("Maps")).unwrap();
        std::fs::create_dir_all(template_dir.join("Media")).unwrap();
        std::fs::write(template_dir.join("Media").join("TP_Code.png"), "png").unwrap();
        std::fs::write(
            template_dir.join("Content").join("Maps").join("Lvl.umap"),
            "map",
        )
        .unwrap();
        std::fs::write(
            template_dir.join("TP_Code.uproject"),
            "{\"FileVersion\": 3, \"EngineAssociation\": \"\", \"Modules\": [{\"Name\": \"TP_Code\"}]}",
        )
        .unwrap();
        std::fs::write(
            source_dir.join("TP_Code.h"),
            "#pragma once\n#define TP_CODE_API\n",
        )
        .unwrap();
        std::fs::write(
            source_dir.join("TP_CodeCharacter.h"),
            "#include \"TP_CodeCharacter.generated.h\"\nclass TP_CODE_API ATP_CodeCharacter {};\n",
        )
        .unwrap();
        std::fs::write(
            template_dir.join("Source").join("TP_Code.Target.cs"),
            "DefaultBuildSettings = BuildSettingsVersion.Latest; ExtraModuleNames.Add(\"TP_Code\");",
        )
        .unwrap();
        std::fs::write(
            template_dir.join("Config").join("DefaultEngine.ini"),
            "[/Script/Engine.RendererSettings]\r\nr.ReflectionMethod=0\r\n",
        )
        .unwrap();
        std::fs::write(
            template_dir.join("Config").join("DefaultGame.ini"),
            "[/Script/EngineSettings.GeneralProjectSettings]\r\nProjectID=OLD\r\n",
        )
        .unwrap();
        let defs_text = THIRD_PERSON_DEFS.replace(
            "LevelPrototyping\",DetailLevels=(\"High\")",
            "Proto\",DetailLevels=(\"Standard\",\"High\")",
        );
        std::fs::write(
            template_dir.join("Config").join("TemplateDefs.ini"),
            &defs_text,
        )
        .unwrap();
        let mut defs = parse_template_defs(&defs_text);
        defs.shared_content_packs
            .retain(|pack| pack.mount_name == "Proto");
        let pack_root = root
            .join("Templates")
            .join("TemplateResources")
            .join("High")
            .join("Proto");
        std::fs::create_dir_all(pack_root.join("Content").join("Meshes")).unwrap();
        std::fs::create_dir_all(pack_root.join("FeaturePack")).unwrap();
        std::fs::write(
            pack_root
                .join("Content")
                .join("Meshes")
                .join("SM_Cube.uasset"),
            "cube",
        )
        .unwrap();
        std::fs::write(
            pack_root.join("FeaturePack").join("manifest.json"),
            "{\"AdditionalFiles\": {\"DestinationFilesFolder\": \"Proto\", \"AdditionalFilesList\": [\"Templates/TemplateResources/High/Proto/Content/*.*\"]}}",
        )
        .unwrap();
        Template {
            folder: template_dir.clone(),
            uproject: template_dir.join("TP_Code.uproject"),
            name: "TP_Code".into(),
            defs,
            generates_code: false,
            thumbnail: None,
            preview: None,
        }
    }

    #[test]
    fn creates_a_project_from_a_fixture_template() {
        let engine = tempfile::tempdir().unwrap();
        let projects = tempfile::tempdir().unwrap();
        let template = fixture_engine(engine.path());
        let request = CreationRequest {
            engine_root: engine.path().to_path_buf(),
            engine_identifier: "5.8".into(),
            engine_version: Some(EngineVersion {
                major: 5,
                minor: 8,
                patch: 0,
            }),
            template,
            project_name: "MyGame".into(),
            location: projects.path().to_path_buf(),
            hardware: HardwareClass::Desktop,
            graphics: GraphicsPreset::Maximum,
        };
        let mut stages = Vec::new();
        let created = create_project(&request, &AtomicBool::new(false), &mut |stage| {
            stages.push(stage)
        })
        .unwrap();
        let root = projects.path().join("MyGame");
        assert_eq!(created.project_file, root.join("MyGame.uproject"));
        assert!(stages.contains(&CreationStage::CopyingSharedContent));

        let descriptor = std::fs::read_to_string(&created.project_file).unwrap();
        assert!(descriptor.contains("\"EngineAssociation\": \"5.8\""));
        assert!(descriptor.contains("\"Name\": \"MyGame\""));
        assert!(!root.join("TP_Code.uproject").exists());
        assert!(!root.join("Config").join("TemplateDefs.ini").exists());
        assert!(!root.join("Media").exists());
        assert!(root.join("Content").join("Maps").join("Lvl.umap").is_file());
        assert!(
            root.join("Content")
                .join("Proto")
                .join("Meshes")
                .join("SM_Cube.uasset")
                .is_file()
        );

        let header =
            std::fs::read_to_string(root.join("Source").join("MyGame").join("MyGameCharacter.h"))
                .unwrap();
        assert_eq!(
            header,
            "#include \"MyGameCharacter.generated.h\"\nclass MYGAME_API AMyGameCharacter {};\n"
        );
        let target = std::fs::read_to_string(root.join("Source").join("MyGame.Target.cs")).unwrap();
        assert!(target.contains("ExtraModuleNames.Add(\"MyGame\")"));
        assert!(target.contains("BuildSettingsVersion.Latest"));

        let engine_ini =
            std::fs::read_to_string(root.join("Config").join("DefaultEngine.ini")).unwrap();
        assert!(engine_ini.contains("r.ReflectionMethod=1"));
        assert!(!engine_ini.contains("r.ReflectionMethod=0"));
        assert!(engine_ini.contains("TargetedHardwareClass=Desktop"));
        assert!(engine_ini.contains("DefaultGraphicsPerformance=Maximum"));
        assert!(engine_ini.contains("r.Substrate=True"));
        assert!(engine_ini.contains("+ActiveClassRedirects=(OldClassName=\"TP_CodeCharacter\",NewClassName=\"MyGameCharacter\")"));
        assert!(engine_ini.contains(
            "+ActiveGameNameRedirects=(OldGameName=\"TP_Code\",NewGameName=\"/Script/MyGame\")"
        ));
        let game_ini =
            std::fs::read_to_string(root.join("Config").join("DefaultGame.ini")).unwrap();
        assert!(!game_ini.contains("ProjectID=OLD"));
        assert!(game_ini.contains("ProjectID="));
    }

    #[test]
    fn cancellation_removes_the_partial_project() {
        let engine = tempfile::tempdir().unwrap();
        let projects = tempfile::tempdir().unwrap();
        let request = CreationRequest {
            engine_root: engine.path().to_path_buf(),
            engine_identifier: "5.8".into(),
            engine_version: None,
            template: fixture_engine(engine.path()),
            project_name: "Cancelled".into(),
            location: projects.path().to_path_buf(),
            hardware: HardwareClass::Mobile,
            graphics: GraphicsPreset::Scalable,
        };
        let error = create_project(&request, &AtomicBool::new(true), &mut |_| {}).unwrap_err();
        assert!(error.is::<Cancelled>());
        assert!(!projects.path().join("Cancelled").exists());
    }

    #[test]
    #[ignore = "creates real projects from the installed engine named by UNREAL_HUB_TEST_ENGINE"]
    fn creates_projects_from_installed_engine() {
        let engine_root = PathBuf::from(std::env::var("UNREAL_HUB_TEST_ENGINE").unwrap());
        let output = PathBuf::from(std::env::var("UNREAL_HUB_TEST_OUTPUT").unwrap());
        let version = crate::unreal::read_build_version(&engine_root).unwrap();
        let catalog = crate::templates::load_catalog(&engine_root).unwrap();
        for (key, use_cpp) in [
            ("TP_Blank", false),
            ("TP_ThirdPerson", false),
            ("TP_ThirdPerson", true),
        ] {
            let group = catalog.group(key).unwrap();
            let template = if use_cpp {
                group.cpp.clone()
            } else {
                group.blueprint.clone()
            }
            .unwrap();
            let name = format!(
                "Hub{}{}",
                key.trim_start_matches("TP_"),
                if use_cpp { "Cpp" } else { "BP" }
            );
            let request = CreationRequest {
                engine_root: engine_root.clone(),
                engine_identifier: format!("{}.{}", version.major, version.minor),
                engine_version: Some(version),
                template,
                project_name: suggest_project_name(&output, &name),
                location: output.clone(),
                hardware: HardwareClass::Desktop,
                graphics: GraphicsPreset::Maximum,
            };
            let created = create_project(&request, &AtomicBool::new(false), &mut |stage| {
                eprintln!("{}: {}", request.project_name, stage.label())
            })
            .unwrap();
            eprintln!(
                "created {} ({:?})",
                created.project_file.display(),
                created.project_files_error
            );
        }
    }
}
