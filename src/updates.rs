use anyhow::{Context as _, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const REPOSITORY: &str = "Mossworm/unreal-hub";
const INSTALLER_SUFFIX: &str = "-setup.exe";
const USER_AGENT: &str = "UnrealHub-Updater";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub fn parse_version(text: &str) -> Option<Version> {
    let trimmed = text.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
    })
}

pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
    })
}

pub fn repository_url() -> String {
    format!("https://github.com/{REPOSITORY}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub page_url: String,
    pub installer_name: String,
    pub installer_url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

pub fn parse_latest_release(json: &str) -> Result<Release> {
    #[derive(Deserialize)]
    struct ReleaseResponse {
        tag_name: String,
        #[serde(default)]
        html_url: String,
        #[serde(default)]
        assets: Vec<AssetResponse>,
    }

    #[derive(Deserialize)]
    struct AssetResponse {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        size: Option<u64>,
        #[serde(default)]
        digest: Option<String>,
    }

    let response: ReleaseResponse =
        serde_json::from_str(json).context("Release response is not valid JSON")?;
    let version = parse_version(&response.tag_name).ok_or_else(|| {
        tr_anyhow!(
            "릴리스 버전을 해석할 수 없습니다: {}",
            "Cannot parse the release version: {}",
            response.tag_name
        )
    })?;
    let asset = response
        .assets
        .into_iter()
        .find(|asset| asset.name.to_ascii_lowercase().ends_with(INSTALLER_SUFFIX))
        .ok_or_else(|| {
            tr_anyhow!(
                "릴리스에 설치 파일이 없습니다",
                "The release has no installer"
            )
        })?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(|hash| hash.to_ascii_lowercase());
    Ok(Release {
        version,
        page_url: response.html_url,
        installer_name: asset.name,
        installer_url: asset.browser_download_url,
        size: asset.size,
        sha256,
    })
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

pub fn fetch_latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let body = agent(Duration::from_secs(20))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context(tr!(
            "업데이트 정보를 가져오지 못했습니다",
            "Failed to fetch update information"
        ))?
        .body_mut()
        .read_to_string()
        .context(tr!(
            "업데이트 정보를 읽지 못했습니다",
            "Failed to read update information"
        ))?;
    parse_latest_release(&body)
}

pub fn update_directory() -> PathBuf {
    std::env::temp_dir().join("UnrealHub").join("updates")
}

fn safe_file_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub fn download_installer(
    release: &Release,
    directory: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    std::fs::create_dir_all(directory).with_context(|| {
        trf!(
            "{} 폴더를 만들 수 없습니다",
            "Cannot create folder {}",
            directory.display()
        )
    })?;
    let destination = directory.join(safe_file_name(&release.installer_name));
    let partial = directory.join(format!("{}.part", safe_file_name(&release.installer_name)));
    let mut hasher = Sha256::new();
    download_to_partial(
        &release.installer_url,
        &partial,
        release.size,
        cancel,
        progress,
        &mut |chunk| hasher.update(chunk),
    )?;
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if let Some(expected) = &release.sha256
        && *expected != actual
    {
        remove_quietly(&partial);
        return Err(tr_anyhow!(
            "설치 파일의 체크섬이 일치하지 않습니다",
            "The installer checksum does not match"
        ));
    }
    replace_with_partial(&partial, &destination)?;
    Ok(destination)
}

pub fn download_file(
    url: &str,
    destination: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    let mut partial_name = destination.as_os_str().to_owned();
    partial_name.push(".part");
    let partial = PathBuf::from(partial_name);
    download_to_partial(url, &partial, None, cancel, progress, &mut |_| {})?;
    replace_with_partial(&partial, destination)
}

fn download_to_partial(
    url: &str,
    partial: &Path,
    expected_size: Option<u64>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
    on_chunk: &mut dyn FnMut(&[u8]),
) -> Result<()> {
    let mut response = agent(Duration::from_secs(30 * 60))
        .get(url)
        .call()
        .context(tr!(
            "설치 파일을 내려받지 못했습니다",
            "Failed to download the installer"
        ))?;
    let total = response.body().content_length().or(expected_size);
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut file = std::fs::File::create(partial).with_context(|| {
        trf!(
            "{}을(를) 쓸 수 없습니다",
            "Cannot write {}",
            partial.display()
        )
    })?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            remove_quietly(partial);
            return Err(tr_anyhow!("다운로드를 취소했습니다", "Download cancelled"));
        }
        let read = reader.read(&mut buffer).context(tr!(
            "설치 파일을 내려받지 못했습니다",
            "Failed to download the installer"
        ))?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).unwrap_or_default();
        file.write_all(chunk)?;
        on_chunk(chunk);
        received += read as u64;
        progress(received, total);
    }
    file.sync_all()?;
    Ok(())
}

fn replace_with_partial(partial: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    std::fs::rename(partial, destination)?;
    Ok(())
}

fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        log::debug!("Cannot remove {}: {error}", path.display());
    }
}

pub fn launch_installer(installer: &Path) -> Result<()> {
    Command::new(installer)
        .args([
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_child| ())
        .context(tr!(
            "설치 파일을 실행하지 못했습니다",
            "Failed to start the installer"
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(
            parse_version("v1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            parse_version("1.0"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(
            parse_version("v2.0.0-beta.1"),
            Some(Version {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(parse_version("latest"), None);
        assert!(parse_version("v1.0.1") > parse_version("1.0.0"));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }

    #[test]
    fn parses_release_response() {
        let json = r#"{
            "tag_name": "v1.0.0",
            "html_url": "https://github.com/Mossworm/unreal-hub/releases/tag/v1.0.0",
            "assets": [
                {"name": "UnrealHub-1.0.0.zip", "browser_download_url": "https://example.invalid/zip", "size": 10},
                {"name": "UnrealHub-1.0.0-setup.exe", "browser_download_url": "https://example.invalid/setup.exe", "size": 1234, "digest": "sha256:ABCDEF"}
            ]
        }"#;
        let release = parse_latest_release(json).unwrap();
        assert_eq!(release.version, parse_version("1.0.0").unwrap());
        assert_eq!(release.installer_name, "UnrealHub-1.0.0-setup.exe");
        assert_eq!(release.installer_url, "https://example.invalid/setup.exe");
        assert_eq!(release.size, Some(1234));
        assert_eq!(release.sha256.as_deref(), Some("abcdef"));
    }

    #[test]
    fn rejects_release_without_installer() {
        let json = r#"{"tag_name": "v1.0.0", "assets": []}"#;
        assert!(parse_latest_release(json).is_err());
    }

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(
            safe_file_name("../Unreal Hub-1.0.0-setup.exe"),
            ".._Unreal_Hub-1.0.0-setup.exe"
        );
    }
}
