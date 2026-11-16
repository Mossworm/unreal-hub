use anyhow::{Context as _, Result};
use futures::channel::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub const RECENT_PROJECT_LIMIT: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraySignal {
    IconClicked,
    MenuItem(MenuId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    OpenProject(String),
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentProject {
    pub key: String,
    pub name: String,
}

pub fn install_event_handlers() -> mpsc::UnboundedReceiver<TraySignal> {
    let (sender, receiver) = mpsc::unbounded();
    let icon_sender = sender.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        let clicked = matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        );
        if clicked && icon_sender.unbounded_send(TraySignal::IconClicked).is_err() {
            log::debug!("Tray receiver closed");
        }
    }));
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if sender
            .unbounded_send(TraySignal::MenuItem(event.id))
            .is_err()
        {
            log::debug!("Tray receiver closed");
        }
    }));
    receiver
}

pub struct Tray {
    icon: TrayIcon,
    recent: Vec<RecentProject>,
    project_items: Vec<(MenuId, String)>,
    show_id: MenuId,
    quit_id: MenuId,
}

struct BuiltMenu {
    menu: Menu,
    project_items: Vec<(MenuId, String)>,
    show_id: MenuId,
    quit_id: MenuId,
}

fn build_menu(recent: &[RecentProject]) -> Result<BuiltMenu> {
    let menu = Menu::new();
    let mut project_items = Vec::new();
    for project in recent.iter().take(RECENT_PROJECT_LIMIT) {
        let item = MenuItem::new(&project.name, true, None);
        menu.append(&item)?;
        project_items.push((item.id().clone(), project.key.clone()));
    }
    if !project_items.is_empty() {
        menu.append(&PredefinedMenuItem::separator())?;
    }
    let show = MenuItem::new(tr!("Unreal Hub 열기", "Open Unreal Hub"), true, None);
    let quit = MenuItem::new(tr!("종료", "Quit"), true, None);
    menu.append(&show)?;
    menu.append(&quit)?;
    Ok(BuiltMenu {
        menu,
        project_items,
        show_id: show.id().clone(),
        quit_id: quit.id().clone(),
    })
}

impl Tray {
    pub fn new(recent: Vec<RecentProject>) -> Result<Self> {
        let built = build_menu(&recent)?;
        let icon = Icon::from_resource(1, None).context("Cannot load the tray icon")?;
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Unreal Hub")
            .with_icon(icon)
            .with_menu(Box::new(built.menu))
            .with_menu_on_left_click(false)
            .build()
            .context("Cannot create the tray icon")?;
        Ok(Self {
            icon: tray_icon,
            recent,
            project_items: built.project_items,
            show_id: built.show_id,
            quit_id: built.quit_id,
        })
    }

    pub fn update_recent(&mut self, recent: Vec<RecentProject>) -> Result<()> {
        if recent == self.recent {
            return Ok(());
        }
        let built = build_menu(&recent)?;
        self.icon.set_menu(Some(Box::new(built.menu)));
        self.recent = recent;
        self.project_items = built.project_items;
        self.show_id = built.show_id;
        self.quit_id = built.quit_id;
        Ok(())
    }

    pub fn refresh_labels(&mut self) -> Result<()> {
        let recent = std::mem::take(&mut self.recent);
        self.recent = vec![RecentProject {
            key: String::new(),
            name: String::new(),
        }];
        self.update_recent(recent)
    }

    pub fn command_for(&self, signal: &TraySignal) -> Option<TrayCommand> {
        match signal {
            TraySignal::IconClicked => Some(TrayCommand::ShowWindow),
            TraySignal::MenuItem(id) if *id == self.show_id => Some(TrayCommand::ShowWindow),
            TraySignal::MenuItem(id) if *id == self.quit_id => Some(TrayCommand::Quit),
            TraySignal::MenuItem(id) => self
                .project_items
                .iter()
                .find(|(item_id, _)| item_id == id)
                .map(|(_, key)| TrayCommand::OpenProject(key.clone())),
        }
    }
}
