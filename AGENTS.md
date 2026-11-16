# AGENTS.md

Guidance for AI coding agents working on this repository.

## Project

A desktop launcher for Unreal Engine projects, similar in spirit to Unity Hub, written in Rust with the GPUI UI framework.

Core responsibilities:

- Discover Unreal projects on disk and keep a persistent project list
- Discover installed engines (launcher installs and source builds)
- Match each project to its engine and launch it
- Show project metadata (name, engine version, path, last opened, thumbnail)

Before changing behavior, read the existing code for the area you touch and follow its patterns.

## Workflow

- Build: `cargo build`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt`
- Test: `cargo test`
- Run all four before declaring a task done. Fix warnings you introduced.
- Keep changes scoped to the task. Do not refactor unrelated code or add features nobody asked for.
- If the GPUI API in this repo differs from what is described here, the code in this repo wins. Check the pinned GPUI version in `Cargo.toml` before using an API you are unsure about.

## Rust conventions

- Correctness and readability first. Optimize only when asked or when profiling shows a need.
- Do not write code comments. Express intent through naming and structure. Doc comments on public action types are the only exception, because GPUI shows them to users.
- Use descriptive, full-word names. No single-letter or abbreviated variable names outside trivial closures.
- Prefer extending an existing module over creating many tiny files.
- Use `src/module_name.rs` files instead of `mod.rs`.
- No `unwrap()`, `expect()`, or other panicking calls in non-test code. Propagate errors with `?` and `anyhow::Context`.
- Treat slice and vector indexing as a possible panic. Prefer `get()` and iterators.
- Never drop a `Result` with `let _ =`. Either propagate it, log it explicitly, or handle the error branch.
- Failures in async work that the user triggered must reach the UI as a visible error, not only a log line.
- In async code, clone what a future needs inside a small block right before the `async move`, so borrows stay short.

## GPUI

### Contexts and windows

- `App` is the root context with access to globals and all entities.
- `Context<T>` is what you get while updating `Entity<T>`. It derefs to `App`, so it can be passed where `&App` is expected.
- `AsyncApp` and `AsyncWindowContext` come from spawned tasks and can live across `.await`.
- `Window` holds per-window state such as focus and action dispatch.
- Parameter order and naming: `window` before `cx`, callbacks last. Always name the context `cx`.

### Entities

- `Entity<T>` is a handle. Use `read(cx)` for a shared reference, `read_with` to compute a value, `update` to mutate, and `update_in` when the closure also needs the `Window`.
- Inside an `update` closure, use the `cx` the closure receives, never the outer one.
- Never update an entity from inside its own update. It panics.
- In async contexts these calls return `anyhow::Result`. Handle it.
- Hold `WeakEntity<T>` when two entities refer to each other, or when a task outlives the view. Upgrading may fail because the entity may be gone, so handle that case.

### Tasks and concurrency

- UI and entity state live on one foreground thread.
- `cx.spawn(async move |cx| ...)` runs on the foreground thread. Inside `Context<T>` the closure receives `this: WeakEntity<T>` first.
- Put file system scans, directory walks, ini and JSON parsing, registry reads, and process launches in `cx.background_spawn`, then apply results on the foreground with `this.update(...)`.
- Dropping a `Task` cancels it. Either store it in a field, await it, or call `detach()` / `detach_and_log_err(cx)` on purpose. Prefer storing it so the work stops when the owner is dropped.
- Use `Task::ready(value)` for an already-known result.

### Rendering

- Views implement `Render`. Small stateless pieces implement `RenderOnce` and derive `IntoElement`.
- Use the Tailwind-like style builders. Use `.when(...)` and `.when_some(...)` for conditional UI instead of branching the whole tree.
- Call `cx.notify()` after any state change that affects what is rendered.

### Input, actions, events

- Wrap handlers that mutate the current view in `cx.listener(...)`.
- Declare simple actions with `actions!(namespace, [...])` and data-carrying actions with `#[derive(Action)]`. Handle them with `.on_action(...)`.
- Emit entity events with `cx.emit(...)` after `impl EventEmitter<Event> for Type {}`.
- Store `Subscription`s from `cx.subscribe` / `cx.observe` in a `_subscriptions: Vec<Subscription>` field. Dropping a subscription unregisters it.

### Tests

- Use GPUI's executor timers in tests (`cx.background_executor().timer(duration).await`) instead of `smol::Timer::after`, so `run_until_parked()` sees the pending work.

## Domain rules

### External files are read-only

The hub reads Epic and engine files but must never write to them unless a task explicitly asks for it. Writing while the Epic Games Launcher runs gets overwritten and can corrupt the user's setup.

Sources to read:

- Epic Games Launcher project roots: `%LOCALAPPDATA%\EpicGamesLauncher\Saved\Config\Windows\GameUserSettings.ini`, section `[Launcher]`, repeated `CreatedProjectPaths=` keys. Each value is a root folder; projects are one level below it.
- Editor recent projects: `%LOCALAPPDATA%\UnrealEngine\<version>\Saved\Config\WindowsEditor\EditorSettings.ini`, key `RecentlyOpenedProjectFiles`. UE4 stores plain path strings; UE5 stores struct strings containing `ProjectName=` and `LastOpenTime=`. Parse both.
- Launcher-installed engines: `C:\ProgramData\Epic\UnrealEngineLauncher\LauncherInstalled.dat` (JSON despite the extension).
- Source-build engines: registry `HKEY_CURRENT_USER\Software\Epic Games\Unreal Engine\Builds` on Windows, `~/.config/Epic/UnrealEngine/Install.ini` on Linux.

Verify each format against real files before relying on it, and keep each parser isolated behind its own function with tests that use fixture files.

### Hub-owned state

- The hub keeps its own project list and user-added root folders in its own config file. That file is the source of truth for anything the user adds or removes in the hub.
- Merge all sources into one list and deduplicate by canonicalized `.uproject` path.
- A project whose path is temporarily unreachable (unplugged drive, sleeping disk) is marked missing, never removed automatically.

### Projects and engines

- A `.uproject` is JSON. Read `EngineAssociation` to match the project to an engine: a version string such as `5.4` for launcher installs, or a GUID / custom identifier for source builds.
- Scanning never walks the whole disk. Only scan registered roots, one level deep, unless the user explicitly asks for a deeper scan.
- Launch editors as detached processes and never block the UI thread while waiting for them.

### Platform

- Windows is the primary target. Keep platform-specific code behind `cfg(target_os = ...)` in dedicated modules so other platforms can be added later.
- Handle paths with `PathBuf`, never string concatenation. Expect spaces, non-ASCII characters, and both slash styles in paths read from ini files.
