use anyhow::{Context as _, Result, bail};
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS, INFINITE, OpenProcess,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows_sys::core::BOOL;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE};

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
    let (builds_key, _) = user
        .create_subkey(SOURCE_BUILDS_KEY)
        .context("레지스트리 키를 열 수 없습니다")?;
    let value = engine_root.to_string_lossy().replace('\\', "/");
    builds_key
        .set_value(identifier, &value)
        .context("엔진을 레지스트리에 등록하지 못했습니다")
}

pub fn unregister_source_build(identifier: &str) -> Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let builds_key = user
        .open_subkey_with_flags(SOURCE_BUILDS_KEY, KEY_SET_VALUE)
        .context("레지스트리 키를 열 수 없습니다")?;
    builds_key
        .delete_value(identifier)
        .context("레지스트리에서 엔진 등록을 해제하지 못했습니다")
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

pub fn has_cpp_toolchain() -> bool {
    let Some(installer) = std::env::var_os("ProgramFiles(x86)").map(|root| {
        PathBuf::from(root)
            .join("Microsoft Visual Studio")
            .join("Installer")
            .join("vswhere.exe")
    }) else {
        return false;
    };
    if !installer.is_file() {
        return false;
    }
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
