//! Application menu (Dock app) and menu-bar item.

use anyhow::{Context, Result};
use tray_icon::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Menu item ids.
pub mod ids {
    /// Show the next page.
    pub const NEXT: &str = "next-page";
    /// Show the previous page.
    pub const PREVIOUS: &str = "previous-page";
    /// Reload `layout.json` and the widgets.
    pub const RELOAD: &str = "reload";
    /// Open the widget configurator.
    pub const CONFIGURE: &str = "configure";
    /// Open the data folder in Finder.
    pub const OPEN_DATA: &str = "open-data";
    /// Quit.
    pub const QUIT: &str = "quit";
}

/// Menus that must stay alive while the app runs.
pub struct Menus {
    _app_menu: Menu,
    _tray: TrayIcon,
    page_label: MenuItem,
}

impl Menus {
    /// Install the application menu and the menu-bar item. Call once the event loop has started.
    ///
    /// # Errors
    ///
    /// If the menu-bar item cannot be created.
    pub fn install() -> Result<Self> {
        let app_menu = Menu::new();
        let app_sub = Submenu::new("EdgeHost", true);
        let about = PredefinedMenuItem::about(
            Some("About EdgeHost"),
            Some(AboutMetadata {
                name: Some("EdgeHost".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                comments: Some(
                    "iCUE HTML widgets on the XENEON EDGE. Not affiliated with CORSAIR.".into(),
                ),
                ..AboutMetadata::default()
            }),
        );
        app_sub
            .append_items(&[
                &about,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id(ids::CONFIGURE, "Configure Widgets…", true, None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ])
            .context("building the application menu")?;
        app_menu
            .append(&app_sub)
            .context("building the application menu")?;
        #[cfg(target_os = "macos")]
        app_menu.init_for_nsapp();

        let page_label = MenuItem::new("No widgets", false, None);
        let tray_menu = Menu::new();
        tray_menu
            .append_items(&[
                &page_label,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id(ids::CONFIGURE, "Configure Widgets…", true, None),
                &MenuItem::with_id(ids::NEXT, "Next Page", true, None),
                &MenuItem::with_id(ids::PREVIOUS, "Previous Page", true, None),
                &MenuItem::with_id(ids::RELOAD, "Reload Layout", true, None),
                &MenuItem::with_id(ids::OPEN_DATA, "Open Data Folder", true, None),
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id(ids::QUIT, "Quit EdgeHost", true, None),
            ])
            .context("building the menu-bar menu")?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_icon_templated(icon()?)
            .with_tooltip("EdgeHost")
            .build()
            .context("creating the menu-bar item")?;
        Ok(Self {
            _app_menu: app_menu,
            _tray: tray,
            page_label,
        })
    }

    /// Show which page is displayed.
    pub fn set_page(&self, page: usize, pages: usize) {
        let text = if pages == 0 {
            "No widgets".to_owned()
        } else {
            format!("Page {} of {pages}", page + 1)
        };
        self.page_label.set_text(text);
    }
}

/// An 18×18 template icon: the Edge's wide panel outline with a dot.
fn icon() -> Result<Icon> {
    const SIZE: usize = 18;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % SIZE, i / SIZE);
        let in_frame = (1..=16).contains(&x) && (5..=12).contains(&y);
        let in_hole = (3..=14).contains(&x) && (7..=10).contains(&y);
        let in_dot = (5..=6).contains(&x) && (8..=9).contains(&y);
        if (in_frame && !in_hole) || in_dot {
            px.copy_from_slice(&[0, 0, 0, 255]);
        }
    }
    let side = u32::try_from(SIZE).context("icon size")?;
    Icon::from_rgba(rgba, side, side).context("building the menu-bar icon")
}
