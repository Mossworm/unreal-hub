use crate::toolchain::{
    ToolVersion, ToolchainRequirements, ToolchainStatus, VisualStudioInstance, evaluate,
    parse_vswhere_instances,
};
use anyhow::{Context as _, Result, bail};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, GetLastError, HANDLE, HWND, LPARAM,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateMutexW, DETACHED_PROCESS, GetExitCodeProcess,
    INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow,
};
use windows_sys::core::BOOL;
use winreg::RegKey;
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_32KEY,
};

pub const BUILD_PLATFORM: &str = "Win64";

const WINDOWS_KITS_KEY: &str = r"SOFTWARE\Microsoft\Windows Kits\Installed Roots";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "UnrealHub";
const SINGLE_INSTANCE_MUTEX: &str = "Local\\UnrealHub-SingleInstance";
const LAUNCHER_REGISTRY_KEY: &str = r"SOFTWARE\EpicGames\Unreal Engine";
const SOURCE_BUILDS_KEY: &str = r"Software\Epic Games\Unreal Engine\Builds";

pub const EDITOR_CONFIG_PLATFORM: &str = "WindowsEditor";

pub fn launcher_installed_dat_path() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|program_data| {
        PathBuf::from(program_data)
            .join("Epic")
            .join("UnrealEngineLauncher")
            .join("LauncherInstalled.dat")
    })
}

pub fn launcher_registry_installs() -> Vec<(String, PathBuf)> {
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(engines_key) = machine.open_subkey(LAUNCHER_REGISTRY_KEY) else {
        return Vec::new();
    };
    engines_key
        .enum_keys()
        .flatten()
        .filter_map(|version| {
            let version_key = engines_key.open_subkey(&version).ok()?;
            let directory: String = version_key.get_value("InstalledDirectory").ok()?;
            let directory = directory.trim();
            (!directory.is_empty()).then(|| (version.clone(), PathBuf::from(directory)))
        })
        .collect()
}

pub fn source_build_registrations() -> Vec<(String, PathBuf)> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(builds_key) = user.open_subkey(SOURCE_BUILDS_KEY) else {
        return Vec::new();
    };
    builds_key
        .enum_values()
        .flatten()
        .filter_map(|(identifier, _)| {
            let directory: String = builds_key.get_value(&identifier).ok()?;
            let directory = directory.trim();
            (!directory.is_empty()).then(|| (identifier.clone(), PathBuf::from(directory)))
        })
        .collect()
}

pub fn is_source_build_registered(identifier: &str) -> bool {
    let normalized = crate::unreal::normalize_identifier(identifier);
    source_build_registrations()
        .iter()
        .any(|(registered, _)| crate::unreal::normalize_identifier(registered) == normalized)
}

pub fn register_source_build(identifier: &str, engine_root: &Path) -> Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let (builds_key, _) = user.create_subkey(SOURCE_BUILDS_KEY).context(tr!(
        "레지스트리 키를 열 수 없습니다",
        "Cannot open the registry key"
    ))?;
    let value = engine_root.to_string_lossy().replace('\\', "/");
    builds_key.set_value(identifier, &value).context(tr!(
        "엔진을 레지스트리에 등록하지 못했습니다",
        "Failed to register the engine in the registry"
    ))
}

pub fn unregister_source_build(identifier: &str) -> Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let builds_key = user
        .open_subkey_with_flags(SOURCE_BUILDS_KEY, KEY_SET_VALUE)
        .context(tr!(
            "레지스트리 키를 열 수 없습니다",
            "Cannot open the registry key"
        ))?;
    builds_key.delete_value(identifier).context(tr!(
        "레지스트리에서 엔진 등록을 해제하지 못했습니다",
        "Failed to unregister the engine from the registry"
    ))
}

pub fn unreal_engine_local_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|local| local.join("UnrealEngine"))
}

pub fn epic_game_user_settings_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|local| {
        local
            .join("EpicGamesLauncher")
            .join("Saved")
            .join("Config")
            .join("Windows")
            .join("GameUserSettings.ini")
    })
}

pub fn editor_binary_candidates(engine_root: &Path) -> Vec<PathBuf> {
    let binaries = engine_root.join("Engine").join("Binaries").join("Win64");
    vec![
        binaries.join("UnrealEditor.exe"),
        binaries.join("UE4Editor.exe"),
    ]
}

pub fn detect_epic_launcher() -> Option<PathBuf> {
    let roots = ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(|variable| std::env::var_os(variable).map(PathBuf::from));
    roots
        .flat_map(|root| {
            let portal = root
                .join("Epic Games")
                .join("Launcher")
                .join("Portal")
                .join("Binaries");
            [
                portal.join("Win64").join("EpicGamesLauncher.exe"),
                portal.join("Win32").join("EpicGamesLauncher.exe"),
            ]
        })
        .find(|candidate| candidate.is_file())
}

pub fn configure_detached(command: &mut Command) {
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

pub fn configure_hidden(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

pub fn unreal_build_tool(engine_root: &Path) -> Option<PathBuf> {
    let path = engine_root
        .join("Engine")
        .join("Binaries")
        .join("DotNET")
        .join("UnrealBuildTool")
        .join("UnrealBuildTool.exe");
    path.is_file().then_some(path)
}

fn visual_studio_installer_directory() -> Option<PathBuf> {
    std::env::var_os("ProgramFiles(x86)").map(|root| {
        PathBuf::from(root)
            .join("Microsoft Visual Studio")
            .join("Installer")
    })
}

fn vswhere_path() -> Option<PathBuf> {
    visual_studio_installer_directory()
        .map(|directory| directory.join("vswhere.exe"))
        .filter(|path| path.is_file())
}

pub fn visual_studio_setup() -> Option<PathBuf> {
    visual_studio_installer_directory()
        .map(|directory| directory.join("setup.exe"))
        .filter(|path| path.is_file())
}

fn visual_studio_instances() -> Vec<VisualStudioInstance> {
    let Some(vswhere) = vswhere_path() else {
        return Vec::new();
    };
    let mut command = Command::new(vswhere);
    command.args(["-products", "*", "-format", "json", "-utf8"]);
    configure_hidden(&mut command);
    let output = match command.output() {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            log::warn!("vswhere exited with {}", output.status);
            return Vec::new();
        }
        Err(error) => {
            log::warn!("vswhere failed: {error}");
            return Vec::new();
        }
    };
    let summaries = match parse_vswhere_instances(&String::from_utf8_lossy(&output.stdout)) {
        Ok(summaries) => summaries,
        Err(error) => {
            log::warn!("{error:#}");
            return Vec::new();
        }
    };
    summaries
        .into_iter()
        .map(|summary| VisualStudioInstance {
            msvc_versions: msvc_versions(&summary.install_path),
            install_path: summary.install_path,
            version: summary.version,
            display_name: summary.display_name,
        })
        .collect()
}

fn msvc_versions(install_path: &Path) -> Vec<ToolVersion> {
    let tools = install_path.join("VC").join("Tools").join("MSVC");
    let Ok(entries) = std::fs::read_dir(&tools) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .join("bin")
                .join("Hostx64")
                .join("x64")
                .join("cl.exe")
                .is_file()
        })
        .filter_map(|entry| ToolVersion::parse(&entry.file_name().to_string_lossy()))
        .collect()
}

fn windows_kits_roots() -> Vec<PathBuf> {
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut roots: Vec<PathBuf> = [KEY_READ, KEY_READ | KEY_WOW64_32KEY]
        .into_iter()
        .filter_map(|flags| {
            let key = machine
                .open_subkey_with_flags(WINDOWS_KITS_KEY, flags)
                .ok()?;
            let root: String = key.get_value("KitsRoot10").ok()?;
            let root = root.trim();
            (!root.is_empty()).then(|| PathBuf::from(root))
        })
        .collect();
    if let Some(program_files) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(program_files).join("Windows Kits").join("10"));
    }
    roots.dedup();
    roots
}

fn windows_sdk_versions() -> Vec<ToolVersion> {
    let mut versions: Vec<ToolVersion> = windows_kits_roots()
        .iter()
        .flat_map(|root| {
            let include = root.join("Include");
            let library = root.join("Lib");
            std::fs::read_dir(&include)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(move |entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let has_headers = entry.path().join("um").join("Windows.h").is_file();
                    let has_libraries = library.join(&name).join("um").join("x64").is_dir();
                    if has_headers && has_libraries {
                        ToolVersion::parse(&name)
                    } else {
                        None
                    }
                })
        })
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

pub fn inspect_toolchain(requirements: &ToolchainRequirements) -> ToolchainStatus {
    evaluate(
        requirements,
        visual_studio_instances(),
        &windows_sdk_versions(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevatedRun {
    Exited(i32),
    Declined,
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

pub fn run_elevated(
    executable: &Path,
    parameters: &str,
    working_directory: &Path,
) -> Result<ElevatedRun> {
    let verb = wide(OsStr::new("runas"));
    let file = wide(executable.as_os_str());
    let parameters = wide(OsStr::new(parameters));
    let directory = wide(working_directory.as_os_str());
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.lpDirectory = directory.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    let started = unsafe { ShellExecuteExW(&mut info) };
    if started == 0 {
        let error = unsafe { GetLastError() };
        if error == ERROR_CANCELLED {
            return Ok(ElevatedRun::Declined);
        }
        tr_bail!(
            "{} 실행에 실패했습니다 (오류 {error})",
            "Failed to start {} (error {error})",
            executable.display()
        );
    }
    if info.hProcess.is_null() {
        bail!("{} started without a process handle", executable.display());
    }
    let mut exit_code = 0u32;
    let read = unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let read = GetExitCodeProcess(info.hProcess, &mut exit_code);
        CloseHandle(info.hProcess);
        read
    };
    if read == 0 {
        bail!("Cannot read the exit code of {}", executable.display());
    }
    Ok(ElevatedRun::Exited(exit_code as i32))
}

pub fn verify_microsoft_signature(path: &Path) -> Result<()> {
    let powershell = std::env::var_os("SystemRoot")
        .map(|root| {
            PathBuf::from(root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!("powershell.exe not found"))?;
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$signature = Get-AuthenticodeSignature -LiteralPath $env:UNREAL_HUB_SIGNED_FILE; \
             [Console]::Out.Write([string]$signature.Status + '|' + $signature.SignerCertificate.Subject)",
        ])
        .env("UNREAL_HUB_SIGNED_FILE", path);
    configure_hidden(&mut command);
    let output = command
        .output()
        .context("Cannot run Get-AuthenticodeSignature")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (status, subject) = text.trim().split_once('|').unwrap_or((text.trim(), ""));
    if output.status.success() && is_trusted_microsoft_signature(status, subject) {
        return Ok(());
    }
    log::error!(
        "Signature check failed for {}: {}",
        path.display(),
        text.trim()
    );
    tr_bail!(
        "{}의 Microsoft 서명을 확인할 수 없습니다",
        "Cannot verify the Microsoft signature of {}",
        path.display()
    )
}

fn is_trusted_microsoft_signature(status: &str, subject: &str) -> bool {
    status.trim() == "Valid"
        && subject
            .split(',')
            .any(|part| part.trim() == "O=Microsoft Corporation")
}

pub fn has_cpp_toolchain() -> bool {
    let Some(installer) = vswhere_path() else {
        return false;
    };
    let mut command = Command::new(installer);
    command.args([
        "-products",
        "*",
        "-requires",
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-property",
        "installationPath",
    ]);
    configure_hidden(&mut command);
    match command.output() {
        Ok(output) => {
            output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
        }
        Err(error) => {
            log::warn!("vswhere failed: {error}");
            false
        }
    }
}

pub fn wait_for_process_exit(pid: u32) -> Result<()> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        bail!("process {pid} is not accessible");
    }
    unsafe {
        WaitForSingleObject(handle, INFINITE);
        CloseHandle(handle);
    }
    Ok(())
}

struct WindowSearch {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn find_process_window(window: HWND, state: LPARAM) -> BOOL {
    let search = unsafe { &mut *(state as *mut WindowSearch) };
    let mut window_pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut window_pid) };
    let is_main_window = unsafe {
        window_pid == search.pid
            && IsWindowVisible(window) != 0
            && GetWindow(window, GW_OWNER).is_null()
    };
    if is_main_window {
        search.found = window;
        0
    } else {
        1
    }
}

pub fn bring_process_window_to_front(pid: u32) -> bool {
    let mut search = WindowSearch {
        pid,
        found: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(
            Some(find_process_window),
            &mut search as *mut WindowSearch as LPARAM,
        );
    }
    if search.found.is_null() {
        return false;
    }
    unsafe {
        if IsIconic(search.found) != 0 {
            ShowWindow(search.found, SW_RESTORE);
        }
        SetForegroundWindow(search.found) != 0
    }
}

pub fn set_launch_at_startup(enabled: bool, executable: &Path) -> Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = user.create_subkey(RUN_KEY).context(tr!(
        "레지스트리 키를 열 수 없습니다",
        "Cannot open the registry key"
    ))?;
    if enabled {
        let command = format!(
            "\"{}\" {}",
            executable.display(),
            crate::platform::AUTOSTART_ARGUMENT
        );
        run_key.set_value(RUN_VALUE, &command).context(tr!(
            "Windows 시작 프로그램에 등록하지 못했습니다",
            "Failed to register Unreal Hub to start with Windows"
        ))
    } else {
        match run_key.delete_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context(tr!(
                "Windows 시작 프로그램 등록을 해제하지 못했습니다",
                "Failed to stop Unreal Hub from starting with Windows"
            )),
        }
    }
}

fn window_hwnd(window: &impl HasWindowHandle) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

pub fn set_window_visible(window: &impl HasWindowHandle, visible: bool) {
    let Some(hwnd) = window_hwnd(window) else {
        log::warn!("Cannot change window visibility: no Win32 handle");
        return;
    };
    unsafe {
        if visible {
            let command = if IsIconic(hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            ShowWindow(hwnd, command);
            SetForegroundWindow(hwnd);
        } else {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

pub struct SingleInstance {
    handle: HANDLE,
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub struct AlreadyRunning;

pub fn acquire_single_instance() -> Result<Option<SingleInstance>, AlreadyRunning> {
    let name: Vec<u16> = SINGLE_INSTANCE_MUTEX
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Ok(None);
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        return Err(AlreadyRunning);
    }
    Ok(Some(SingleInstance { handle }))
}

struct TitleSearch {
    title: Vec<u16>,
    own_pid: u32,
    found: HWND,
}

unsafe extern "system" fn find_window_by_title(window: HWND, state: LPARAM) -> BOOL {
    let search = unsafe { &mut *(state as *mut TitleSearch) };
    let mut window_pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut window_pid) };
    if window_pid == search.own_pid || unsafe { !GetWindow(window, GW_OWNER).is_null() } {
        return 1;
    }
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    let text = buffer
        .get(..usize::try_from(length).unwrap_or(0))
        .unwrap_or_default();
    if text == search.title.as_slice() {
        search.found = window;
        0
    } else {
        1
    }
}

pub fn activate_existing_instance(title: &str) -> bool {
    let mut search = TitleSearch {
        title: title.encode_utf16().collect(),
        own_pid: std::process::id(),
        found: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(
            Some(find_window_by_title),
            &mut search as *mut TitleSearch as LPARAM,
        );
    }
    if search.found.is_null() {
        return false;
    }
    unsafe {
        let command = if IsIconic(search.found) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindow(search.found, command);
        SetForegroundWindow(search.found) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_valid_microsoft_signatures() {
        let subject =
            "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";
        assert!(is_trusted_microsoft_signature("Valid", subject));
        assert!(!is_trusted_microsoft_signature("HashMismatch", subject));
        assert!(!is_trusted_microsoft_signature(
            "Valid",
            "CN=Microsoft Corporation Fake, O=Contoso"
        ));
        assert!(!is_trusted_microsoft_signature("NotSigned", ""));
    }
}
