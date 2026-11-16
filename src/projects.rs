use crate::engines::{Engine, EngineResolution, resolve_association};
use crate::paths::{clean_path, display_path, path_key, volume_root};
use crate::platform;
use crate::unreal::{
    AssociationKind, EngineVersion, RecentProject, UProjectInfo, classify_association, is_uproject,
    parse_created_project_paths, parse_recent_projects, project_name, read_uproject,
};
use anyhow::{Context as _, Result};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

pub const WATCH_FOLDER_DEPTH: usize = 3;
pub const EPIC_CREATED_ROOT_DEPTH: usize = 1;

const SKIPPED_DIRECTORY_NAMES: [&str; 12] = [
    "Binaries",
    "Intermediate",
    "Saved",
    "DerivedDataCache",
    "Content",
    "Config",
    "Source",
    "Plugins",
    "Build",
    ".git",
    ".vs",
    "node_modules",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectOrigin {
    #[default]
    Manual,
    WatchFolder,
    EngineRecent,
    EpicLauncher,
    Created,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectPresence {
    #[default]
    Present,
    Unreachable,
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub path: PathBuf,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub last_opened: Option<i64>,
    #[serde(default)]
    pub origin: ProjectOrigin,
    #[serde(default)]
    pub engine_association: Option<String>,
    #[serde(default)]
    pub is_cpp: bool,
    #[serde(default)]
    pub presence: ProjectPresence,
}

impl ProjectRecord {
    pub fn new(path: PathBuf, origin: ProjectOrigin) -> Self {
        Self {
            path,
            favorite: false,
            last_opened: None,
            origin,
            engine_association: None,
            is_cpp: false,
            presence: ProjectPresence::Present,
        }
    }

    pub fn key(&self) -> String {
        path_key(&self.path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInspection {
    pub key: String,
    pub path: PathBuf,
    pub presence: ProjectPresence,
    pub info: Option<UProjectInfo>,
}

pub fn presence_of(path: &Path) -> ProjectPresence {
    if path.is_file() {
        ProjectPresence::Present
    } else if volume_root(path).is_some_and(|root| root.exists()) {
        ProjectPresence::Gone
    } else {
        ProjectPresence::Unreachable
    }
}

pub fn inspect_project(path: &Path) -> ProjectInspection {
    let presence = presence_of(path);
    let info = if presence == ProjectPresence::Present {
        match read_uproject(path) {
            Ok(info) => Some(info),
            Err(error) => {
                log::warn!("{error:#}");
                None
            }
        }
    } else {
        None
    };
    ProjectInspection {
        key: path_key(path),
        path: path.to_path_buf(),
        presence,
        info,
    }
}

pub fn find_uprojects(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    scan_directory(root, max_depth + 1, &mut found);
    found
}

fn scan_directory(directory: &Path, remaining_depth: usize, found: &mut Vec<PathBuf>) {
    if remaining_depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut subdirectories = Vec::new();
    let mut found_here = false;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file() && is_uproject(&path) {
            found.push(path);
            found_here = true;
        } else if file_type.is_dir() && !is_skipped_directory(&entry.file_name().to_string_lossy())
        {
            subdirectories.push(path);
        }
    }
    if found_here {
        return;
    }
    for subdirectory in subdirectories {
        scan_directory(&subdirectory, remaining_depth - 1, found);
    }
}

pub fn is_skipped_directory(name: &str) -> bool {
    SKIPPED_DIRECTORY_NAMES
        .iter()
        .any(|skipped| skipped.eq_ignore_ascii_case(name))
}

pub fn collect_dropped_projects(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut projects = Vec::new();
    for path in paths {
        if path.is_dir() {
            projects.extend(find_uprojects(path, WATCH_FOLDER_DEPTH));
        } else if is_uproject(path) && path.is_file() {
            projects.push(path.clone());
        }
    }
    projects.iter().map(|path| clean_path(path)).collect()
}

pub fn read_engine_recent_projects() -> Vec<RecentProject> {
    let Some(base) = platform::unreal_engine_local_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    for entry in entries.flatten() {
        let settings_path = entry
            .path()
            .join("Saved")
            .join("Config")
            .join(platform::EDITOR_CONFIG_PLATFORM)
            .join("EditorSettings.ini");
        if let Ok(text) = std::fs::read_to_string(&settings_path) {
            projects.extend(parse_recent_projects(&text));
        }
    }
    projects
}

pub fn read_epic_created_project_roots() -> Vec<PathBuf> {
    let Some(path) = platform::epic_game_user_settings_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_created_project_paths(&text),
        Err(error) => {
            log::debug!("Cannot read {}: {error}", path.display());
            Vec::new()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRow {
    pub key: String,
    pub path: PathBuf,
    pub name: String,
    pub folder: String,
    pub favorite: bool,
    pub last_opened: Option<i64>,
    pub association: Option<String>,
    pub resolution: Option<EngineResolution>,
    pub version_sort_key: Option<EngineVersion>,
    pub presence: ProjectPresence,
    pub is_cpp: bool,
}

impl ProjectRow {
    pub fn from_record(record: &ProjectRecord, engines: &[Engine]) -> Self {
        let resolution = record
            .engine_association
            .as_deref()
            .map(|association| resolve_association(association, &record.path, engines));
        let version_sort_key = resolution
            .as_ref()
            .and_then(|resolution| resolution.engine_key())
            .and_then(|key| engines.iter().find(|engine| engine.key == key))
            .and_then(|engine| engine.version)
            .or_else(|| {
                record
                    .engine_association
                    .as_deref()
                    .and_then(|association| match classify_association(association) {
                        AssociationKind::Version { major, minor } => Some(EngineVersion {
                            major,
                            minor,
                            patch: 0,
                        }),
                        _ => None,
                    })
            });
        let folder = record.path.parent().map(display_path).unwrap_or_default();
        Self {
            key: record.key(),
            path: record.path.clone(),
            name: project_name(&record.path),
            folder,
            favorite: record.favorite,
            last_opened: record.last_opened,
            association: record.engine_association.clone(),
            resolution,
            version_sort_key,
            presence: record.presence,
            is_cpp: record.is_cpp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortColumn {
    Name,
    EngineVersion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSort {
    pub column: Option<SortColumn>,
    pub descending: bool,
}

impl ProjectSort {
    pub fn cycle(self, column: SortColumn) -> Self {
        if self.column != Some(column) {
            Self {
                column: Some(column),
                descending: false,
            }
        } else if !self.descending {
            Self {
                column: Some(column),
                descending: true,
            }
        } else {
            Self::default()
        }
    }
}

pub fn compare_rows(left: &ProjectRow, right: &ProjectRow, sort: ProjectSort) -> Ordering {
    let favorites = right.favorite.cmp(&left.favorite);
    let by_name = || left.name.to_lowercase().cmp(&right.name.to_lowercase());
    let column_order = match sort.column {
        None => right.last_opened.cmp(&left.last_opened).then_with(by_name),
        Some(SortColumn::Name) => {
            let order = by_name();
            if sort.descending {
                order.reverse()
            } else {
                order
            }
        }
        Some(SortColumn::EngineVersion) => {
            let order = left.version_sort_key.cmp(&right.version_sort_key);
            let order = if sort.descending {
                order.reverse()
            } else {
                order
            };
            order.then_with(by_name)
        }
    };
    favorites
        .then(column_order)
        .then_with(|| left.key.cmp(&right.key))
}

pub fn visible_row_indices(rows: &[ProjectRow], sort: ProjectSort, query: &str) -> Vec<usize> {
    let mut indices: Vec<usize> = if query.trim().is_empty() {
        (0..rows.len()).collect()
    } else {
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buffer = Vec::new();
        rows.iter()
            .enumerate()
            .filter(|(_, row)| {
                let name_match = pattern
                    .score(Utf32Str::new(&row.name, &mut buffer), &mut matcher)
                    .is_some();
                name_match
                    || pattern
                        .score(
                            Utf32Str::new(&display_path(&row.path), &mut buffer),
                            &mut matcher,
                        )
                        .is_some()
            })
            .map(|(index, _)| index)
            .collect()
    };
    indices.sort_by(|left, right| match (rows.get(*left), rows.get(*right)) {
        (Some(left_row), Some(right_row)) => compare_rows(left_row, right_row, sort),
        _ => Ordering::Equal,
    });
    indices
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CacheFolder {
    Intermediate,
    DerivedDataCache,
    Saved,
    Binaries,
}

impl CacheFolder {
    pub const ALL: [CacheFolder; 4] = [
        CacheFolder::Intermediate,
        CacheFolder::DerivedDataCache,
        CacheFolder::Saved,
        CacheFolder::Binaries,
    ];

    pub fn directory_name(self) -> &'static str {
        match self {
            CacheFolder::Intermediate => "Intermediate",
            CacheFolder::DerivedDataCache => "DerivedDataCache",
            CacheFolder::Saved => "Saved",
            CacheFolder::Binaries => "Binaries",
        }
    }

    pub fn selected_by_default(self) -> bool {
        matches!(
            self,
            CacheFolder::Intermediate | CacheFolder::DerivedDataCache
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanupOptions {
    pub keep_saved_config: bool,
    pub include_plugin_folders: bool,
}

pub fn cache_folder_targets(
    project_dir: &Path,
    folder: CacheFolder,
    options: CleanupOptions,
) -> Vec<PathBuf> {
    let directory = project_dir.join(folder.directory_name());
    let mut targets = Vec::new();
    match folder {
        CacheFolder::Saved if options.keep_saved_config => {
            if let Ok(entries) = std::fs::read_dir(&directory) {
                targets.extend(
                    entries
                        .flatten()
                        .filter(|entry| !entry.file_name().eq_ignore_ascii_case("Config"))
                        .map(|entry| entry.path()),
                );
            }
        }
        _ => {
            if directory.exists() {
                targets.push(directory);
            }
        }
    }
    if folder == CacheFolder::Binaries && options.include_plugin_folders {
        targets.extend(plugin_build_folders(project_dir));
    }
    targets
}

pub fn plugin_build_folders(project_dir: &Path) -> Vec<PathBuf> {
    let mut plugin_directories = Vec::new();
    collect_plugin_directories(&project_dir.join("Plugins"), 4, &mut plugin_directories);
    plugin_directories
        .into_iter()
        .flat_map(|plugin| [plugin.join("Binaries"), plugin.join("Intermediate")])
        .filter(|path| path.exists())
        .collect()
}

fn collect_plugin_directories(directory: &Path, remaining_depth: usize, found: &mut Vec<PathBuf>) {
    if remaining_depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut subdirectories = Vec::new();
    let mut is_plugin = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("uplugin"))
        {
            is_plugin = true;
        } else if file_type.is_dir() && !is_skipped_directory(&entry.file_name().to_string_lossy())
        {
            subdirectories.push(path);
        }
    }
    if is_plugin {
        found.push(directory.to_path_buf());
        return;
    }
    for subdirectory in subdirectories {
        collect_plugin_directories(&subdirectory, remaining_depth - 1, found);
    }
}

pub fn measure_paths(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .map(|path| {
            if path.is_dir() {
                crate::engines::folder_size(path)
            } else {
                std::fs::metadata(path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
            }
        })
        .sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    Trash,
    Permanent,
}

pub fn delete_paths(paths: &[PathBuf], mode: DeleteMode) -> Result<()> {
    let existing: Vec<&PathBuf> = paths.iter().filter(|path| path.exists()).collect();
    if existing.is_empty() {
        return Ok(());
    }
    match mode {
        DeleteMode::Trash => trash::delete_all(&existing).context(tr!(
            "휴지통으로 이동하지 못했습니다",
            "Failed to move to the Recycle Bin"
        )),
        DeleteMode::Permanent => {
            for path in existing {
                if path.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                }
                .with_context(|| {
                    trf!(
                        "{} 삭제에 실패했습니다",
                        "Failed to delete {}",
                        path.display()
                    )
                })?;
            }
            Ok(())
        }
    }
}

pub fn write_uproject_association(
    project_path: &Path,
    association: &str,
    create_backup: bool,
) -> Result<()> {
    let text = std::fs::read_to_string(project_path).with_context(|| {
        trf!(
            "{}을(를) 읽을 수 없습니다",
            "Cannot read {}",
            project_path.display()
        )
    })?;
    let updated = crate::unreal::set_engine_association(&text, association)?;
    if create_backup {
        let mut backup = project_path.as_os_str().to_owned();
        backup.push(".bak");
        std::fs::copy(project_path, PathBuf::from(backup)).context(tr!(
            ".uproject 백업을 만들지 못했습니다",
            "Failed to back up the .uproject"
        ))?;
    }
    let mut temporary = project_path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, updated).context(tr!(
        ".uproject를 쓰지 못했습니다",
        "Failed to write the .uproject"
    ))?;
    std::fs::rename(&temporary, project_path).context(tr!(
        ".uproject를 교체하지 못했습니다",
        "Failed to replace the .uproject"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        name: &str,
        favorite: bool,
        last_opened: Option<i64>,
        version: Option<(u32, u32)>,
    ) -> ProjectRow {
        ProjectRow {
            key: name.to_lowercase(),
            path: PathBuf::from(format!("/projects/{name}/{name}.uproject")),
            name: name.into(),
            folder: format!("/projects/{name}"),
            favorite,
            last_opened,
            association: None,
            resolution: None,
            version_sort_key: version.map(|(major, minor)| EngineVersion {
                major,
                minor,
                patch: 0,
            }),
            presence: ProjectPresence::Present,
            is_cpp: false,
        }
    }

    fn names(rows: &[ProjectRow], indices: &[usize]) -> Vec<String> {
        indices
            .iter()
            .map(|index| rows[*index].name.clone())
            .collect()
    }

    #[test]
    fn default_sort_puts_favorites_first_then_recent() {
        let rows = vec![
            row("Alpha", false, Some(10), None),
            row("Bravo", true, Some(1), None),
            row("Charlie", false, Some(30), None),
            row("Delta", false, None, None),
        ];
        let indices = visible_row_indices(&rows, ProjectSort::default(), "");
        assert_eq!(
            names(&rows, &indices),
            vec!["Bravo", "Charlie", "Alpha", "Delta"]
        );
    }

    #[test]
    fn sorts_by_engine_version_keeping_favorites_pinned() {
        let rows = vec![
            row("Alpha", false, None, Some((5, 9))),
            row("Bravo", false, None, Some((5, 5))),
            row("Charlie", true, None, Some((5, 6))),
        ];
        let ascending = ProjectSort::default().cycle(SortColumn::EngineVersion);
        assert_eq!(
            names(&rows, &visible_row_indices(&rows, ascending, "")),
            vec!["Charlie", "Bravo", "Alpha"]
        );
        let descending = ascending.cycle(SortColumn::EngineVersion);
        assert_eq!(
            names(&rows, &visible_row_indices(&rows, descending, "")),
            vec!["Charlie", "Alpha", "Bravo"]
        );
        assert_eq!(
            descending.cycle(SortColumn::EngineVersion),
            ProjectSort::default()
        );
    }

    #[test]
    fn fuzzy_filters_by_name_and_path() {
        let rows = vec![
            row("MyShooter", false, None, None),
            row("OldPrototype", false, None, None),
        ];
        let indices = visible_row_indices(&rows, ProjectSort::default(), "mshtr");
        assert_eq!(names(&rows, &indices), vec!["MyShooter"]);
        let indices = visible_row_indices(&rows, ProjectSort::default(), "zzz");
        assert!(indices.is_empty());
    }

    #[test]
    fn filters_one_thousand_rows_quickly() {
        let rows: Vec<_> = (0..1000)
            .map(|index| {
                row(
                    &format!("Project{index}"),
                    index % 7 == 0,
                    Some(index),
                    None,
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let indices = visible_row_indices(&rows, ProjectSort::default(), "proj99");
        assert!(!indices.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
    }

    #[test]
    fn finds_projects_within_depth_and_skips_project_internals() {
        let root = tempfile::tempdir().unwrap();
        let shallow = root.path().join("Shallow");
        let deep = root.path().join("a").join("b").join("Deep");
        let too_deep = root.path().join("a").join("b").join("c").join("TooDeep");
        for directory in [&shallow, &deep, &too_deep] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(shallow.join("Shallow.uproject"), "{}").unwrap();
        std::fs::create_dir_all(shallow.join("Plugins").join("Nested")).unwrap();
        std::fs::write(
            shallow
                .join("Plugins")
                .join("Nested")
                .join("Nested.uproject"),
            "{}",
        )
        .unwrap();
        std::fs::write(deep.join("Deep.uproject"), "{}").unwrap();
        std::fs::write(too_deep.join("TooDeep.uproject"), "{}").unwrap();

        let mut found: Vec<String> = find_uprojects(root.path(), WATCH_FOLDER_DEPTH)
            .iter()
            .map(|path| project_name(path))
            .collect();
        found.sort();
        assert_eq!(found, vec!["Deep", "Shallow"]);
    }

    #[test]
    fn reports_presence() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("Game.uproject");
        std::fs::write(&project, "{}").unwrap();
        assert_eq!(presence_of(&project), ProjectPresence::Present);
        assert_eq!(
            presence_of(&root.path().join("Missing.uproject")),
            ProjectPresence::Gone
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reports_unreachable_volume() {
        let missing_drive = ('D'..='Z')
            .map(|letter| format!(r"{letter}:\"))
            .find(|root| !Path::new(root).exists())
            .unwrap();
        let project = PathBuf::from(missing_drive)
            .join("Game")
            .join("Game.uproject");
        assert_eq!(presence_of(&project), ProjectPresence::Unreachable);
    }

    #[test]
    fn cleanup_targets_respect_options() {
        let project = tempfile::tempdir().unwrap();
        let saved = project.path().join("Saved");
        std::fs::create_dir_all(saved.join("Config")).unwrap();
        std::fs::create_dir_all(saved.join("Logs")).unwrap();
        std::fs::create_dir_all(project.path().join("Binaries")).unwrap();
        let plugin = project.path().join("Plugins").join("Tool");
        std::fs::create_dir_all(plugin.join("Intermediate")).unwrap();
        std::fs::write(plugin.join("Tool.uplugin"), "{}").unwrap();

        let keep = CleanupOptions {
            keep_saved_config: true,
            include_plugin_folders: true,
        };
        assert_eq!(
            cache_folder_targets(project.path(), CacheFolder::Saved, keep),
            vec![saved.join("Logs")]
        );
        assert_eq!(
            cache_folder_targets(project.path(), CacheFolder::Binaries, keep),
            vec![project.path().join("Binaries"), plugin.join("Intermediate")]
        );
        assert!(cache_folder_targets(project.path(), CacheFolder::Intermediate, keep).is_empty());

        let all = CleanupOptions {
            keep_saved_config: false,
            include_plugin_folders: false,
        };
        assert_eq!(
            cache_folder_targets(project.path(), CacheFolder::Saved, all),
            vec![saved.clone()]
        );
        delete_paths(
            &cache_folder_targets(project.path(), CacheFolder::Saved, all),
            DeleteMode::Permanent,
        )
        .unwrap();
        assert!(!saved.exists());
    }

    #[test]
    fn writes_association_with_backup() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("Game.uproject");
        let original = "{\n\t\"FileVersion\": 3,\n\t\"EngineAssociation\": \"5.6\"\n}";
        std::fs::write(&project, original).unwrap();
        write_uproject_association(&project, "5.8", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&project).unwrap(),
            original.replace("5.6", "5.8")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("Game.uproject.bak")).unwrap(),
            original
        );
    }
}
