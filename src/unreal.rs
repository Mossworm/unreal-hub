use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EngineVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for EngineVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub fn build_version_path(engine_root: &Path) -> PathBuf {
    engine_root
        .join("Engine")
        .join("Build")
        .join("Build.version")
}

pub fn parse_build_version(text: &str) -> Result<EngineVersion> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct BuildVersionFile {
        major_version: u32,
        minor_version: u32,
        #[serde(default)]
        patch_version: u32,
    }

    let file: BuildVersionFile =
        serde_json::from_str(strip_bom(text)).context("Build.version is not valid JSON")?;
    Ok(EngineVersion {
        major: file.major_version,
        minor: file.minor_version,
        patch: file.patch_version,
    })
}

pub fn read_build_version(engine_root: &Path) -> Result<EngineVersion> {
    let path = build_version_path(engine_root);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_build_version(&text).with_context(|| format!("Failed to parse {}", path.display()))
}

pub fn parse_major_minor(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherInstall {
    pub association: String,
    pub install_location: PathBuf,
}

pub fn parse_launcher_installed(text: &str) -> Result<Vec<LauncherInstall>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct LauncherInstalledFile {
        #[serde(default)]
        installation_list: Vec<LauncherInstalledEntry>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct LauncherInstalledEntry {
        #[serde(default)]
        install_location: String,
        #[serde(default)]
        app_name: String,
    }

    let file: LauncherInstalledFile =
        serde_json::from_str(strip_bom(text)).context("LauncherInstalled.dat is not valid JSON")?;
    Ok(file
        .installation_list
        .into_iter()
        .filter_map(|entry| {
            let association = entry.app_name.strip_prefix("UE_")?;
            parse_major_minor(association)?;
            if entry.install_location.trim().is_empty() {
                return None;
            }
            Some(LauncherInstall {
                association: association.to_string(),
                install_location: PathBuf::from(entry.install_location.trim()),
            })
        })
        .collect())
}

pub fn ini_values(text: &str, section: &str, key: &str) -> Vec<String> {
    let mut in_section = false;
    let mut values = Vec::new();
    for raw_line in strip_bom(text).lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_section = name.trim().eq_ignore_ascii_case(section);
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((raw_key, value)) = line.split_once('=') else {
            continue;
        };
        let line_key = raw_key.trim().trim_start_matches(['+', '-', '.', '!']);
        if line_key.eq_ignore_ascii_case(key) {
            values.push(value.trim().to_string());
        }
    }
    values
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentProject {
    pub path: PathBuf,
    pub last_open_time: Option<i64>,
}

pub fn parse_recent_projects(text: &str) -> Vec<RecentProject> {
    ini_values(
        text,
        "/Script/UnrealEd.EditorSettings",
        "RecentlyOpenedProjectFiles",
    )
    .iter()
    .filter_map(|value| parse_recent_project_value(value))
    .collect()
}

fn parse_recent_project_value(value: &str) -> Option<RecentProject> {
    let trimmed = value.trim();
    if let Some(inner) = trimmed
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let fields = parse_struct_fields(inner);
        let project_name = struct_field(&fields, "ProjectName")?;
        let last_open_time =
            struct_field(&fields, "LastOpenTime").and_then(|time| parse_unreal_timestamp(&time));
        return recent_project_path(&project_name).map(|path| RecentProject {
            path,
            last_open_time,
        });
    }
    recent_project_path(trimmed.trim_matches('"')).map(|path| RecentProject {
        path,
        last_open_time: None,
    })
}

fn recent_project_path(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    let is_uproject = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("uproject"));
    (path.is_absolute() && is_uproject).then_some(path)
}

fn parse_struct_fields(inner: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for character in inner.chars() {
        match character {
            '"' => {
                in_quotes = !in_quotes;
                current.push(character);
            }
            ',' if !in_quotes => {
                push_struct_field(&mut fields, &current);
                current.clear();
            }
            _ => current.push(character),
        }
    }
    push_struct_field(&mut fields, &current);
    fields
}

fn push_struct_field(fields: &mut Vec<(String, String)>, field: &str) {
    if let Some((key, value)) = field.split_once('=') {
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(value);
        fields.push((key.trim().to_string(), value.to_string()));
    }
}

fn struct_field(fields: &[(String, String)], name: &str) -> Option<String> {
    fields
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

pub fn parse_unreal_timestamp(text: &str) -> Option<i64> {
    let (date, time) = text.trim().split_once('-')?;
    let mut date_parts = date.split('.').map(|part| part.parse::<u32>().ok());
    let year = date_parts.next()??;
    let month = date_parts.next()??;
    let day = date_parts.next()??;
    let mut time_parts = time.split('.').map(|part| part.parse::<u32>().ok());
    let hour = time_parts.next()??;
    let minute = time_parts.next()??;
    let second = time_parts.next()??;
    let valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60
        && second < 61;
    if !valid {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    Some(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn parse_created_project_paths(text: &str) -> Vec<PathBuf> {
    ini_values(text, "Launcher", "CreatedProjectPaths")
        .into_iter()
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UProjectInfo {
    pub engine_association: String,
    pub is_cpp: bool,
}

pub fn parse_uproject(text: &str) -> Result<UProjectInfo> {
    let value: serde_json::Value =
        serde_json::from_str(strip_bom(text)).context(".uproject is not valid JSON")?;
    if !value.is_object() {
        bail!(".uproject root is not a JSON object");
    }
    let engine_association = value
        .get("EngineAssociation")
        .and_then(|association| association.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let is_cpp = value
        .get("Modules")
        .and_then(|modules| modules.as_array())
        .is_some_and(|modules| !modules.is_empty());
    Ok(UProjectInfo {
        engine_association,
        is_cpp,
    })
}

pub fn read_uproject(path: &Path) -> Result<UProjectInfo> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_uproject(&text).with_context(|| format!("Failed to parse {}", path.display()))
}

pub fn set_engine_association(text: &str, association: &str) -> Result<String> {
    parse_uproject(text)?;
    let encoded = serde_json::to_string(association)?;
    let updated = match find_top_level_value(text, "EngineAssociation")? {
        Some(range) => {
            let before = text
                .get(..range.start)
                .ok_or_else(|| anyhow!("invalid value range"))?;
            let after = text
                .get(range.end..)
                .ok_or_else(|| anyhow!("invalid value range"))?;
            format!("{before}{encoded}{after}")
        }
        None => insert_first_key(text, "EngineAssociation", &encoded)?,
    };
    let verified = parse_uproject(&updated)?;
    if verified.engine_association != association.trim() {
        bail!("EngineAssociation was not updated");
    }
    Ok(updated)
}

fn find_top_level_value(text: &str, key: &str) -> Result<Option<Range<usize>>> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut depth = 0usize;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => {
                let end = string_end(bytes, index)?;
                if depth == 1 {
                    let after = skip_whitespace(bytes, end);
                    if bytes.get(after) == Some(&b':') && text.get(index + 1..end - 1) == Some(key)
                    {
                        let value_start = skip_whitespace(bytes, after + 1);
                        let value_end = value_end(bytes, value_start)?;
                        return Ok(Some(value_start..value_end));
                    }
                }
                index = end;
            }
            b'{' | b'[' => {
                depth += 1;
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ => index += 1,
        }
    }
    Ok(None)
}

fn string_end(bytes: &[u8], start: usize) -> Result<usize> {
    let mut index = start + 1;
    loop {
        match bytes.get(index) {
            Some(b'\\') => index += 2,
            Some(b'"') => return Ok(index + 1),
            Some(_) => index += 1,
            None => bail!("unterminated JSON string"),
        }
    }
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        index += 1;
    }
    index
}

fn value_end(bytes: &[u8], start: usize) -> Result<usize> {
    match bytes.get(start) {
        Some(b'"') => string_end(bytes, start),
        Some(b'{') | Some(b'[') => bail!("EngineAssociation is not a scalar value"),
        Some(_) => {
            let mut index = start;
            while let Some(&byte) = bytes.get(index) {
                if byte == b',' || byte == b'}' || byte == b']' || byte.is_ascii_whitespace() {
                    break;
                }
                index += 1;
            }
            Ok(index)
        }
        None => bail!("missing JSON value"),
    }
}

fn insert_first_key(text: &str, key: &str, encoded_value: &str) -> Result<String> {
    let bytes = text.as_bytes();
    let brace = text
        .find('{')
        .ok_or_else(|| anyhow!(".uproject has no JSON object"))?;
    let after_brace = brace + 1;
    let first_token = skip_whitespace(bytes, after_brace);
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let indentation = text
        .get(after_brace..first_token)
        .and_then(|gap| gap.rsplit('\n').next())
        .filter(|indent| !indent.is_empty())
        .unwrap_or("\t")
        .trim_end_matches('\r')
        .to_string();
    let head = text
        .get(..after_brace)
        .ok_or_else(|| anyhow!("invalid object start"))?;
    if bytes.get(first_token) == Some(&b'}') {
        let tail = text
            .get(first_token..)
            .ok_or_else(|| anyhow!("invalid object end"))?;
        return Ok(format!(
            "{head}{line_ending}{indentation}\"{key}\": {encoded_value}{line_ending}{tail}"
        ));
    }
    let tail = text
        .get(after_brace..)
        .ok_or_else(|| anyhow!("invalid object body"))?;
    Ok(format!(
        "{head}{line_ending}{indentation}\"{key}\": {encoded_value},{tail}"
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssociationKind {
    Empty,
    Version { major: u32, minor: u32 },
    Identifier(String),
}

pub fn classify_association(value: &str) -> AssociationKind {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return AssociationKind::Empty;
    }
    match parse_major_minor(trimmed) {
        Some((major, minor)) => AssociationKind::Version { major, minor },
        None => AssociationKind::Identifier(normalize_identifier(trimmed)),
    }
}

pub fn normalize_identifier(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_ascii_uppercase()
}

pub fn new_engine_identifier() -> String {
    format!(
        "{{{}}}",
        uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
    )
}

pub fn project_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub fn is_uproject(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("uproject"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCHER_INSTALLED: &str = include_str!("../tests/fixtures/LauncherInstalled.dat");
    const EDITOR_SETTINGS_UE5: &str = include_str!("../tests/fixtures/EditorSettings_UE5.ini");
    const EDITOR_SETTINGS_UE4: &str = include_str!("../tests/fixtures/EditorSettings_UE4.ini");
    const GAME_USER_SETTINGS: &str = include_str!("../tests/fixtures/GameUserSettings.ini");
    const BUILD_VERSION: &str = include_str!("../tests/fixtures/Build.version");
    const UPROJECT_BLUEPRINT: &str = include_str!("../tests/fixtures/Blueprint.uproject");
    const UPROJECT_CPP: &str = include_str!("../tests/fixtures/Cpp.uproject");

    #[test]
    fn parses_build_version() {
        let version = parse_build_version(BUILD_VERSION).unwrap();
        assert_eq!(
            version,
            EngineVersion {
                major: 5,
                minor: 8,
                patch: 4
            }
        );
        assert_eq!(version.to_string(), "5.8.4");
    }

    #[test]
    fn parses_launcher_installed_engines_only() {
        let installs = parse_launcher_installed(LAUNCHER_INSTALLED).unwrap();
        assert_eq!(
            installs,
            vec![
                LauncherInstall {
                    association: "5.8".into(),
                    install_location: PathBuf::from(r"C:\Program Files\Epic Games\UE_5.8"),
                },
                LauncherInstall {
                    association: "5.6".into(),
                    install_location: PathBuf::from(r"C:\Program Files\Epic Games\UE_5.6"),
                },
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_ue5_recent_projects() {
        let recent = parse_recent_projects(EDITOR_SETTINGS_UE5);
        assert_eq!(recent.len(), 2);
        assert_eq!(
            recent[0].path,
            PathBuf::from("C:/Users/Dev/Unreal Projects/Shooter Game/Shooter.uproject")
        );
        assert_eq!(recent[0].last_open_time, Some(1_794_760_733));
        assert_eq!(
            recent[1].path,
            PathBuf::from("D:/작업/테스트/test.uproject")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_ue4_recent_projects() {
        let recent = parse_recent_projects(EDITOR_SETTINGS_UE4);
        assert_eq!(
            recent,
            vec![
                RecentProject {
                    path: PathBuf::from("D:/Old/OldPrototype/OldPrototype.uproject"),
                    last_open_time: None,
                },
                RecentProject {
                    path: PathBuf::from(r"E:\Legacy Games\Racer\Racer.uproject"),
                    last_open_time: None,
                },
            ]
        );
    }

    #[test]
    fn parses_created_project_paths() {
        assert_eq!(
            parse_created_project_paths(GAME_USER_SETTINGS),
            vec![
                PathBuf::from(r"C:\Users\Dev\Documents\Unreal Projects"),
                PathBuf::from("D:/Projects"),
            ]
        );
    }

    #[test]
    fn parses_unreal_timestamp() {
        assert_eq!(parse_unreal_timestamp("1970.01.01-00.00.00"), Some(0));
        assert_eq!(
            parse_unreal_timestamp("2026.11.15-16.38.53"),
            Some(1_794_760_733)
        );
        assert_eq!(parse_unreal_timestamp("2026.13.15-16.38.53"), None);
        assert_eq!(parse_unreal_timestamp("garbage"), None);
    }

    #[test]
    fn parses_uproject_kind() {
        let blueprint = parse_uproject(UPROJECT_BLUEPRINT).unwrap();
        assert_eq!(blueprint.engine_association, "5.8");
        assert!(!blueprint.is_cpp);
        let cpp = parse_uproject(UPROJECT_CPP).unwrap();
        assert_eq!(
            cpp.engine_association,
            "{8A1B2C3D-0000-4E5F-9A8B-7C6D5E4F3A2B}"
        );
        assert!(cpp.is_cpp);
    }

    #[test]
    fn replaces_engine_association_preserving_format() {
        let updated = set_engine_association(UPROJECT_BLUEPRINT, "5.9").unwrap();
        assert_eq!(
            updated,
            UPROJECT_BLUEPRINT.replace(
                "\"EngineAssociation\": \"5.8\"",
                "\"EngineAssociation\": \"5.9\""
            )
        );
    }

    #[test]
    fn inserts_engine_association_when_missing() {
        let text = "{\r\n\t\"FileVersion\": 3,\r\n\t\"Category\": \"\"\r\n}\r\n";
        let updated = set_engine_association(text, "{ABC}").unwrap();
        assert_eq!(
            updated,
            "{\r\n\t\"EngineAssociation\": \"{ABC}\",\r\n\t\"FileVersion\": 3,\r\n\t\"Category\": \"\"\r\n}\r\n"
        );
    }

    #[test]
    fn inserts_engine_association_into_empty_object() {
        let updated = set_engine_association("{}", "5.8").unwrap();
        assert_eq!(parse_uproject(&updated).unwrap().engine_association, "5.8");
    }

    #[test]
    fn ignores_nested_engine_association_keys() {
        let text = "{\n  \"Plugins\": [{\"EngineAssociation\": \"x\"}],\n  \"EngineAssociation\": \"5.6\"\n}";
        let updated = set_engine_association(text, "5.8").unwrap();
        assert!(updated.contains("[{\"EngineAssociation\": \"x\"}]"));
        assert!(updated.contains("\"EngineAssociation\": \"5.8\"\n}"));
    }

    #[test]
    fn classifies_associations() {
        assert_eq!(classify_association(""), AssociationKind::Empty);
        assert_eq!(
            classify_association("5.9"),
            AssociationKind::Version { major: 5, minor: 9 }
        );
        assert_eq!(
            classify_association("{abc-def}"),
            AssociationKind::Identifier("ABC-DEF".into())
        );
        assert_eq!(
            classify_association("5.9.1"),
            AssociationKind::Identifier("5.9.1".into())
        );
    }

    #[test]
    fn generates_braced_uppercase_identifiers() {
        let identifier = new_engine_identifier();
        assert!(identifier.starts_with('{') && identifier.ends_with('}'));
        assert_eq!(identifier.len(), 38);
        assert_eq!(identifier, identifier.to_uppercase());
    }

    #[test]
    fn reads_ini_values_case_insensitively_with_array_prefixes() {
        let text = "[Section]\n+Key=one\nkey = two\n[Other]\nKey=three\n";
        assert_eq!(ini_values(text, "section", "KEY"), vec!["one", "two"]);
    }
}
