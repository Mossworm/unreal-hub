#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engines;
mod paths;
mod platform;
mod project_creation;
mod projects;
mod state;
mod storage;
mod templates;
mod ui;
mod unreal;
mod util;

use crate::state::HubState;
use crate::storage::{Loaded, LoadedState, Settings};
use crate::ui::hub_view::HubView;
use gpui::{
    App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point,
    px, size,
};
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;

const MINIMUM_WIDTH: f32 = 900.0;
const MINIMUM_HEIGHT: f32 = 560.0;
const DEFAULT_WIDTH: f32 = 1040.0;
const DEFAULT_HEIGHT: f32 = 700.0;

fn init_logging(data_dir: &Path) -> Option<WorkerGuard> {
    let appender = match Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("unreal-hub")
        .filename_suffix("log")
        .max_log_files(7)
        .build(data_dir.join("logs"))
    {
        Ok(appender) => appender,
        Err(error) => {
            eprintln!("Cannot create log file: {error}");
            return None;
        }
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Err(error) = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init()
    {
        eprintln!("Cannot initialize logging: {error}");
    }
    Some(guard)
}

fn fresh<T: Default>() -> Loaded<T> {
    Loaded {
        value: T::default(),
        recovered_from_backup: false,
        error: None,
    }
}

fn empty_state(data_dir: PathBuf) -> LoadedState {
    LoadedState {
        data_dir,
        settings: fresh(),
        projects: fresh(),
        engines: fresh(),
    }
}

fn window_bounds(settings: &Settings, cx: &App) -> WindowBounds {
    let saved = settings.window.and_then(|placement| {
        let bounds = Bounds::new(
            point(px(placement.x), px(placement.y)),
            size(
                px(placement.width.max(MINIMUM_WIDTH)),
                px(placement.height.max(MINIMUM_HEIGHT)),
            ),
        );
        let on_screen = cx
            .displays()
            .iter()
            .any(|display| display.bounds().intersects(&bounds));
        on_screen.then_some(if placement.maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        })
    });
    saved.unwrap_or_else(|| {
        WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT)),
            cx,
        ))
    })
}

fn open_main_window(loaded: LoadedState, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(window_bounds(&loaded.settings.value, cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("Unreal Hub".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        window_min_size: Some(size(px(MINIMUM_WIDTH), px(MINIMUM_HEIGHT))),
        app_id: Some("UnrealHub".into()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let state = cx.new(|cx| HubState::new(loaded, cx));
        cx.new(|cx| HubView::new(state, window, cx))
    });
    match opened {
        Ok(_) => cx.activate(true),
        Err(error) => {
            log::error!("Cannot open the main window: {error:#}");
            cx.quit();
        }
    }
}

fn main() {
    let data_dir = match storage::app_data_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("{error:#}");
            std::env::temp_dir().join("UnrealHub")
        }
    };
    let _log_guard = init_logging(&data_dir);
    log::info!("Unreal Hub {} starting", env!("CARGO_PKG_VERSION"));

    let loader_dir = data_dir.clone();
    let loader = std::thread::Builder::new()
        .name("state-loader".into())
        .spawn(move || storage::load_state(loader_dir));

    Application::new()
        .with_assets(ui::Assets)
        .run(move |cx: &mut App| {
            ui::init(cx);
            let loaded = match loader {
                Ok(handle) => handle.join().unwrap_or_else(|_| {
                    log::error!("State loader panicked");
                    empty_state(data_dir.clone())
                }),
                Err(error) => {
                    log::error!("Cannot spawn state loader: {error}");
                    storage::load_state(data_dir.clone())
                }
            };
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            open_main_window(loaded, cx);
        });
}
