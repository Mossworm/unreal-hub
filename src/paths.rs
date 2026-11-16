use std::path::{Component, Path, PathBuf};

pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub fn clean_path(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(canonical) => strip_verbatim_prefix(canonical),
        Err(_) => lexical_normalize(path),
    }
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

pub fn path_key(path: &Path) -> String {
    let normalized = lexical_normalize(path);
    let text = normalized.to_string_lossy().replace('/', "\\");
    let trimmed = text.trim_end_matches('\\');
    let trimmed = if trimmed.is_empty() {
        text.as_str()
    } else {
        trimmed
    };
    if cfg!(target_os = "windows") {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

pub fn volume_root(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => root.push(component.as_os_str()),
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        None
    } else {
        Some(root)
    }
}

pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(target_os = "windows") {
        text.replace('/', "\\")
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_parent_and_current_components() {
        let path = Path::new("projects/./a/../b/Game.uproject");
        assert_eq!(
            lexical_normalize(path),
            PathBuf::from("projects").join("b").join("Game.uproject")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn path_key_ignores_case_and_slash_style() {
        let forward = path_key(Path::new("C:/Users/Me/Unreal Projects/Game/Game.uproject"));
        let backward = path_key(Path::new(r"c:\users\me\Unreal Projects\Game\Game.uproject"));
        assert_eq!(forward, backward);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn strips_verbatim_prefixes() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\Engines\UE_5.8")),
            PathBuf::from(r"C:\Engines\UE_5.8")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\Game")),
            PathBuf::from(r"\\server\share\Game")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn finds_volume_root() {
        assert_eq!(
            volume_root(Path::new(r"E:\Projects\Game\Game.uproject")),
            Some(PathBuf::from(r"E:\"))
        );
        assert_eq!(volume_root(Path::new(r"relative\Game.uproject")), None);
    }
}
