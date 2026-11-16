use crate::paths::{clean_path, path_key};
use crate::platform;
use crate::unreal::{
    AssociationKind, EngineVersion, classify_association, normalize_identifier,
    parse_launcher_installed, read_build_version,
};
use crate::util::system_time_to_unix;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EngineSource {
    Launcher,
    SourceBuild,
    Manual,
}

impl EngineSource {
    pub fn label(self) -> &'static str {
        match self {
            EngineSource::Launcher => "Launcher",
            EngineSource::SourceBuild => tr!("소스 빌드", "Source build"),
            EngineSource::Manual => tr!("수동", "Manual"),
        }
    }

    fn priority(self) -> u8 {
        match self {
            EngineSource::Launcher => 0,
            EngineSource::SourceBuild => 1,
            EngineSource::Manual => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engine {
    pub key: String,
    pub identifier: String,
    pub source: EngineSource,
    pub root: PathBuf,
    pub version: Option<EngineVersion>,
    pub available: bool,
}

impl Engine {
    pub fn version_label(&self) -> String {
        self.version
            .map(|version| version.to_string())
            .unwrap_or_else(|| tr!("알 수 없음", "Unknown").to_string())
    }

    pub fn label(&self) -> String {
        format!("{} ({})", self.version_label(), self.source.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualEngine {
    pub identifier: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineCandidate {
    pub identifier: String,
    pub source: EngineSource,
    pub root: PathBuf,
}

pub fn gather_candidates(manual_engines: &[ManualEngine]) -> Vec<EngineCandidate> {
    let mut candidates = Vec::new();
    if let Some(path) = platform::launcher_installed_dat_path() {
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse_launcher_installed(&text) {
                Ok(installs) => {
                    candidates.extend(installs.into_iter().map(|install| EngineCandidate {
                        identifier: install.association,
                        source: EngineSource::Launcher,
                        root: install.install_location,
                    }))
                }
                Err(error) => log::warn!("Ignoring {}: {error:#}", path.display()),
            },
            Err(error) => log::info!("Cannot read {}: {error}", path.display()),
        }
    }
    candidates.extend(platform::launcher_registry_installs().into_iter().map(
        |(identifier, root)| EngineCandidate {
            identifier,
            source: EngineSource::Launcher,
            root,
        },
    ));
    candidates.extend(platform::source_build_registrations().into_iter().map(
        |(identifier, root)| EngineCandidate {
            identifier,
            source: EngineSource::SourceBuild,
            root,
        },
    ));
    candidates.extend(manual_engines.iter().map(|engine| EngineCandidate {
        identifier: engine.identifier.clone(),
        source: EngineSource::Manual,
        root: engine.root.clone(),
    }));
    candidates
        .into_iter()
        .map(|candidate| EngineCandidate {
            root: clean_path(&candidate.root),
            ..candidate
        })
        .collect()
}

pub fn detect_engines(manual_engines: &[ManualEngine]) -> Vec<Engine> {
    merge_candidates(
        gather_candidates(manual_engines),
        |root| match read_build_version(root) {
            Ok(version) => Some(version),
            Err(error) => {
                log::debug!("Not an engine root: {error:#}");
                None
            }
        },
    )
}

pub fn merge_candidates(
    mut candidates: Vec<EngineCandidate>,
    read_version: impl Fn(&Path) -> Option<EngineVersion>,
) -> Vec<Engine> {
    candidates.sort_by_key(|candidate| candidate.source.priority());
    let mut seen_keys = HashSet::new();
    let mut engines = Vec::new();
    for candidate in candidates {
        let key = path_key(&candidate.root);
        if seen_keys.contains(&key) {
            continue;
        }
        let version = read_version(&candidate.root);
        if version.is_none() && candidate.source != EngineSource::Manual {
            continue;
        }
        seen_keys.insert(key.clone());
        engines.push(Engine {
            key,
            identifier: candidate.identifier,
            source: candidate.source,
            root: candidate.root,
            available: version.is_some(),
            version,
        });
    }
    sort_engines(&mut engines);
    engines
}

pub fn sort_engines(engines: &mut [Engine]) {
    engines.sort_by(|left, right| {
        right
            .version
            .cmp(&left.version)
            .then(left.source.priority().cmp(&right.source.priority()))
            .then(left.key.cmp(&right.key))
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineResolution {
    Matched(String),
    Embedded(String),
    NotInstalled(String),
    UnknownBuild,
}

impl EngineResolution {
    pub fn engine_key(&self) -> Option<&str> {
        match self {
            EngineResolution::Matched(key) | EngineResolution::Embedded(key) => Some(key),
            EngineResolution::NotInstalled(_) | EngineResolution::UnknownBuild => None,
        }
    }
}

pub fn resolve_association(
    association: &str,
    project_path: &Path,
    engines: &[Engine],
) -> EngineResolution {
    match classify_association(association) {
        AssociationKind::Version { major, minor } => engines
            .iter()
            .filter(|engine| engine.available)
            .filter(|engine| {
                engine
                    .version
                    .is_some_and(|version| version.major == major && version.minor == minor)
            })
            .max_by_key(|engine| (engine.source == EngineSource::Launcher, engine.version))
            .map(|engine| EngineResolution::Matched(engine.key.clone()))
            .unwrap_or_else(|| EngineResolution::NotInstalled(format!("{major}.{minor}"))),
        AssociationKind::Identifier(identifier) => engines
            .iter()
            .filter(|engine| engine.available)
            .find(|engine| normalize_identifier(&engine.identifier) == identifier)
            .map(|engine| EngineResolution::Matched(engine.key.clone()))
            .unwrap_or(EngineResolution::UnknownBuild),
        AssociationKind::Empty => project_path
            .ancestors()
            .skip(1)
            .find_map(|ancestor| {
                let key = path_key(ancestor);
                engines
                    .iter()
                    .find(|engine| engine.available && engine.key == key)
            })
            .map(|engine| EngineResolution::Embedded(engine.key.clone()))
            .unwrap_or(EngineResolution::UnknownBuild),
    }
}

pub fn is_engine_root(path: &Path) -> bool {
    crate::unreal::build_version_path(path).is_file()
}

pub fn folder_size(path: &Path) -> u64 {
    let mut total = 0u64;
    for entry in jwalk::WalkDir::new(path)
        .skip_hidden(false)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file()
            && let Ok(metadata) = entry.metadata()
        {
            total = total.saturating_add(metadata.len());
        }
    }
    total
}

pub fn folder_modified(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(system_time_to_unix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u32, minor: u32, patch: u32) -> EngineVersion {
        EngineVersion {
            major,
            minor,
            patch,
        }
    }

    fn engine(
        source: EngineSource,
        identifier: &str,
        root: &str,
        version: EngineVersion,
    ) -> Engine {
        Engine {
            key: path_key(Path::new(root)),
            identifier: identifier.into(),
            source,
            root: PathBuf::from(root),
            version: Some(version),
            available: true,
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn merges_duplicate_roots_by_source_priority() {
        let candidates = vec![
            EngineCandidate {
                identifier: "{MANUAL}".into(),
                source: EngineSource::Manual,
                root: PathBuf::from(r"c:\program files\epic games\ue_5.8"),
            },
            EngineCandidate {
                identifier: "5.8".into(),
                source: EngineSource::Launcher,
                root: PathBuf::from(r"C:\Program Files\Epic Games\UE_5.8"),
            },
            EngineCandidate {
                identifier: "{BUILD}".into(),
                source: EngineSource::SourceBuild,
                root: PathBuf::from(r"D:\UE\UnrealEngine"),
            },
            EngineCandidate {
                identifier: "4.0".into(),
                source: EngineSource::Launcher,
                root: PathBuf::from(r"C:\Program Files\Epic Games\4.0"),
            },
            EngineCandidate {
                identifier: "{GONE}".into(),
                source: EngineSource::Manual,
                root: PathBuf::from(r"E:\Engines\UE_5.7"),
            },
        ];
        let engines = merge_candidates(candidates, |root| {
            let text = root.to_string_lossy().to_lowercase();
            if text.ends_with("ue_5.8") {
                Some(version(5, 8, 4))
            } else if text.ends_with("unrealengine") {
                Some(version(5, 8, 0))
            } else {
                None
            }
        });
        let summary: Vec<_> = engines
            .iter()
            .map(|engine| (engine.identifier.as_str(), engine.source, engine.available))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("5.8", EngineSource::Launcher, true),
                ("{BUILD}", EngineSource::SourceBuild, true),
                ("{GONE}", EngineSource::Manual, false),
            ]
        );
    }

    #[test]
    fn resolves_version_association_preferring_launcher() {
        let engines = vec![
            engine(
                EngineSource::SourceBuild,
                "{B}",
                "/engines/source",
                version(5, 8, 9),
            ),
            engine(
                EngineSource::Launcher,
                "5.8",
                "/engines/launcher",
                version(5, 8, 3),
            ),
            engine(
                EngineSource::Launcher,
                "5.9",
                "/engines/ue59",
                version(5, 9, 1),
            ),
        ];
        assert_eq!(
            resolve_association("5.8", Path::new("/p/Game.uproject"), &engines),
            EngineResolution::Matched(path_key(Path::new("/engines/launcher")))
        );
        assert_eq!(
            resolve_association("5.5", Path::new("/p/Game.uproject"), &engines),
            EngineResolution::NotInstalled("5.5".into())
        );
    }

    #[test]
    fn resolves_highest_patch_without_launcher() {
        let engines = vec![
            engine(
                EngineSource::SourceBuild,
                "{A}",
                "/engines/a",
                version(5, 7, 1),
            ),
            engine(EngineSource::Manual, "{B}", "/engines/b", version(5, 7, 4)),
        ];
        assert_eq!(
            resolve_association("5.7", Path::new("/p/Game.uproject"), &engines),
            EngineResolution::Matched(path_key(Path::new("/engines/b")))
        );
    }

    #[test]
    fn resolves_identifier_association_ignoring_braces_and_case() {
        let engines = vec![engine(
            EngineSource::SourceBuild,
            "{ABCDEF}",
            "/engines/source",
            version(5, 8, 0),
        )];
        assert_eq!(
            resolve_association("abcdef", Path::new("/p/Game.uproject"), &engines),
            EngineResolution::Matched(path_key(Path::new("/engines/source")))
        );
        assert_eq!(
            resolve_association("{OTHER}", Path::new("/p/Game.uproject"), &engines),
            EngineResolution::UnknownBuild
        );
    }

    #[test]
    fn resolves_embedded_projects_from_ancestors() {
        let engines = vec![engine(
            EngineSource::SourceBuild,
            "{A}",
            "/engines/source",
            version(5, 8, 0),
        )];
        assert_eq!(
            resolve_association(
                "",
                Path::new("/engines/source/Samples/Lyra/Lyra.uproject"),
                &engines
            ),
            EngineResolution::Embedded(path_key(Path::new("/engines/source")))
        );
        assert_eq!(
            resolve_association("", Path::new("/elsewhere/Game.uproject"), &engines),
            EngineResolution::UnknownBuild
        );
    }

    #[test]
    fn sorts_engines_by_version_descending() {
        let mut engines = vec![
            engine(EngineSource::Launcher, "5.6", "/a", version(5, 6, 1)),
            engine(EngineSource::Launcher, "5.8", "/b", version(5, 8, 4)),
            Engine {
                version: None,
                available: false,
                ..engine(EngineSource::Manual, "{X}", "/c", version(0, 0, 0))
            },
        ];
        sort_engines(&mut engines);
        let order: Vec<_> = engines
            .iter()
            .map(|engine| engine.identifier.as_str())
            .collect();
        assert_eq!(order, vec!["5.8", "5.6", "{X}"]);
    }

    #[test]
    fn measures_folder_size() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("nested")).unwrap();
        std::fs::write(directory.path().join("a.bin"), [0u8; 100]).unwrap();
        std::fs::write(directory.path().join("nested").join("b.bin"), [0u8; 50]).unwrap();
        assert_eq!(folder_size(directory.path()), 150);
    }
}
