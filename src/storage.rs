use crate::engines::{Engine, ManualEngine};
use crate::projects::{ProjectRecord, ProjectSort};
use anyhow::{Context as _, Result, anyhow};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const SETTINGS_FILE: &str = "settings.json";
pub const PROJECTS_FILE: &str = "projects.json";
pub const ENGINES_FILE: &str = "engines.json";

pub fn app_data_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|config| config.join("UnrealHub"))
        .ok_or_else(|| anyhow!("Cannot determine the application data directory"))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreference {
    Dark,
    Light,
    #[default]
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    Projects,
    Installs,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub minimize_after_launch: bool,
    pub new_project_directory: Option<PathBuf>,
    pub default_engine: Option<String>,
    pub watch_folders: Vec<PathBuf>,
    pub import_engine_recent_on_startup: bool,
    pub engine_recent_imported: bool,
    pub theme: ThemePreference,
    pub ui_scale: u32,
    pub epic_launcher_path: Option<PathBuf>,
    pub last_tab: Tab,
    pub window: Option<WindowPlacement>,
    pub project_sort: ProjectSort,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            minimize_after_launch: false,
            new_project_directory: None,
            default_engine: None,
            watch_folders: Vec::new(),
            import_engine_recent_on_startup: true,
            engine_recent_imported: false,
            theme: ThemePreference::System,
            ui_scale: 100,
            epic_launcher_path: None,
            last_tab: Tab::Projects,
            window: None,
            project_sort: ProjectSort::default(),
        }
    }
}

impl Settings {
    pub const UI_SCALES: [u32; 6] = [90, 100, 110, 125, 140, 150];

    pub fn ui_scale_factor(&self) -> f32 {
        self.ui_scale.clamp(90, 150) as f32 / 100.0
    }

    pub fn new_project_directory_or_default(&self) -> Option<PathBuf> {
        self.new_project_directory
            .clone()
            .or_else(|| dirs::document_dir().map(|documents| documents.join("Unreal Projects")))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectsFile {
    pub version: u32,
    pub projects: Vec<ProjectRecord>,
}

impl Default for ProjectsFile {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            projects: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSizeRecord {
    pub key: String,
    pub modified: Option<i64>,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnginesFile {
    pub version: u32,
    pub manual: Vec<ManualEngine>,
    pub detected: Vec<Engine>,
    pub sizes: Vec<EngineSizeRecord>,
}

impl Default for EnginesFile {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            manual: Vec::new(),
            detected: Vec::new(),
            sizes: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub recovered_from_backup: bool,
    pub error: Option<String>,
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    PathBuf::from(backup)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    PathBuf::from(temporary)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| format!("Cannot parse {}", path.display()))
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Loaded<T> {
    if !path.exists() && !backup_path(path).exists() {
        return Loaded {
            value: T::default(),
            recovered_from_backup: false,
            error: None,
        };
    }
    match read_json(path) {
        Ok(value) => Loaded {
            value,
            recovered_from_backup: false,
            error: None,
        },
        Err(primary_error) => match read_json(&backup_path(path)) {
            Ok(value) => {
                log::warn!("Recovered from backup: {primary_error:#}");
                Loaded {
                    value,
                    recovered_from_backup: true,
                    error: Some(format!("{primary_error:#}")),
                }
            }
            Err(backup_error) => {
                log::error!("{primary_error:#}; backup: {backup_error:#}");
                Loaded {
                    value: T::default(),
                    recovered_from_backup: false,
                    error: Some(format!("{primary_error:#}")),
                }
            }
        },
    }
}

pub fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    if read_json::<serde_json::Value>(path).is_ok() {
        std::fs::copy(path, backup_path(path))
            .with_context(|| format!("Cannot back up {}", path.display()))?;
    }
    let temporary = temporary_path(path);
    {
        let mut file = std::fs::File::create(&temporary)
            .with_context(|| format!("Cannot create {}", temporary.display()))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path).with_context(|| format!("Cannot replace {}", path.display()))
}

#[derive(Clone)]
pub struct JsonFile {
    path: PathBuf,
    next_generation: u64,
    last_written: Arc<Mutex<u64>>,
}

pub struct PendingWrite {
    path: PathBuf,
    contents: String,
    generation: u64,
    last_written: Arc<Mutex<u64>>,
}

impl JsonFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            next_generation: 0,
            last_written: Arc::new(Mutex::new(0)),
        }
    }

    pub fn prepare<T: Serialize>(&mut self, value: &T) -> Result<PendingWrite> {
        self.next_generation += 1;
        Ok(PendingWrite {
            path: self.path.clone(),
            contents: serde_json::to_string_pretty(value)?,
            generation: self.next_generation,
            last_written: self.last_written.clone(),
        })
    }
}

impl PendingWrite {
    pub fn write(self) -> Result<()> {
        let mut last_written = self
            .last_written
            .lock()
            .map_err(|_| anyhow!("Save lock for {} is poisoned", self.path.display()))?;
        if *last_written > self.generation {
            return Ok(());
        }
        write_atomically(&self.path, &self.contents)?;
        *last_written = self.generation;
        Ok(())
    }
}

pub struct LoadedState {
    pub data_dir: PathBuf,
    pub settings: Loaded<Settings>,
    pub projects: Loaded<ProjectsFile>,
    pub engines: Loaded<EnginesFile>,
}

pub fn load_state(data_dir: PathBuf) -> LoadedState {
    LoadedState {
        settings: load_json(&data_dir.join(SETTINGS_FILE)),
        projects: load_json(&data_dir.join(PROJECTS_FILE)),
        engines: load_json(&data_dir.join(ENGINES_FILE)),
        data_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_loads_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let loaded: Loaded<Settings> = load_json(&directory.path().join(SETTINGS_FILE));
        assert_eq!(loaded.value, Settings::default());
        assert!(loaded.error.is_none());
    }

    #[test]
    fn saves_atomically_and_recovers_from_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SETTINGS_FILE);
        let mut file = JsonFile::new(path.clone());

        let first = Settings {
            ui_scale: 110,
            ..Settings::default()
        };
        file.prepare(&first).unwrap().write().unwrap();
        let second = Settings {
            ui_scale: 125,
            ..Settings::default()
        };
        file.prepare(&second).unwrap().write().unwrap();
        assert_eq!(load_json::<Settings>(&path).value, second);

        std::fs::write(&path, "{ corrupted").unwrap();
        let loaded = load_json::<Settings>(&path);
        assert!(loaded.recovered_from_backup);
        assert_eq!(loaded.value, first);
    }

    #[test]
    fn stale_generation_does_not_overwrite_newer_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SETTINGS_FILE);
        let mut file = JsonFile::new(path.clone());
        let stale = file
            .prepare(&Settings {
                ui_scale: 90,
                ..Settings::default()
            })
            .unwrap();
        let fresh = file
            .prepare(&Settings {
                ui_scale: 150,
                ..Settings::default()
            })
            .unwrap();
        fresh.write().unwrap();
        stale.write().unwrap();
        assert_eq!(load_json::<Settings>(&path).value.ui_scale, 150);
    }

    #[test]
    fn unknown_fields_and_missing_fields_are_tolerated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"version":1,"ui_scale":125,"future_field":true}"#).unwrap();
        let loaded = load_json::<Settings>(&path);
        assert_eq!(loaded.value.ui_scale, 125);
        assert!(loaded.value.import_engine_recent_on_startup);
    }
}
