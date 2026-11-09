//! The native application menu, built with `muda` from the menu model in
//! `desktop_core::menu`.
//!
//! Item ids are the model's ids. A click is posted to the event loop, which
//! asks the core what that item does. Whenever the core pushes a new
//! `app-menu` snapshot to the renderer, [`NativeMenu::sync`] applies the same
//! labels and enabled states here.

use std::collections::HashMap;

use tao::event_loop::EventLoopProxy;

use muda::accelerator::Accelerator;
use muda::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use desktop_core::ipc::protocol::SerializableMenuItem;

use crate::host::AppEvent;

pub struct NativeMenu {
    menu: Menu,
    items: HashMap<String, MenuItem>,
    /// What each item currently shows, to skip redundant native updates.
    shown: HashMap<String, (String, bool)>,
}

/// Build the menu and route clicks into the tao event loop.
pub fn install(proxy: EventLoopProxy<AppEvent>, model: &[SerializableMenuItem]) -> Option<NativeMenu> {
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = proxy.send_event(AppEvent::NativeMenu(event.id.0.clone()));
    }));

    let mut native = NativeMenu {
        menu: Menu::new(),
        items: HashMap::new(),
        shown: HashMap::new(),
    };

    let built = model.iter().try_for_each(|top| {
        let submenu = Submenu::with_id(MenuId::new(&top.id), label(top), top.enabled);
        native.fill(&submenu, &top.submenu)?;
        native.menu.append(&submenu)
    });

    match built {
        Ok(()) => Some(native),
        Err(error) => {
            tracing::warn!(%error, "the native menu could not be built");
            None
        }
    }
}

fn label(item: &SerializableMenuItem) -> &str {
    &item.label
}

impl NativeMenu {
    fn fill(&mut self, parent: &Submenu, children: &[SerializableMenuItem]) -> muda::Result<()> {
        for child in children.iter().filter(|child| child.visible) {
            match child.kind.as_str() {
                "separator" => parent.append(&PredefinedMenuItem::separator())?,
                "submenuItem" => {
                    let submenu = Submenu::with_id(MenuId::new(&child.id), label(child), child.enabled);
                    self.fill(&submenu, &child.submenu)?;
                    parent.append(&submenu)?;
                }
                _ => {
                    let accelerator = child.accelerator.as_deref().and_then(|text| {
                        text.parse::<Accelerator>()
                            .inspect_err(|error| {
                                tracing::debug!(%error, accelerator = text, id = %child.id, "unsupported menu accelerator");
                            })
                            .ok()
                    });
                    let item = MenuItem::with_id(MenuId::new(&child.id), label(child), child.enabled, accelerator);
                    parent.append(&item as &dyn IsMenuItem)?;
                    self.shown.insert(child.id.clone(), (label(child).to_string(), child.enabled));
                    self.items.insert(child.id.clone(), item);
                }
            }
        }
        Ok(())
    }

    /// Apply the labels and enabled states of a new menu snapshot.
    pub fn sync(&mut self, model: &[SerializableMenuItem]) {
        for entry in model {
            if let Some(item) = self.items.get(&entry.id) {
                let wanted = (label(entry).to_string(), entry.enabled);
                let shown = self.shown.get(&entry.id);
                if shown.map(|current| &current.0) != Some(&wanted.0) {
                    item.set_text(&wanted.0);
                }
                if shown.map(|current| current.1) != Some(wanted.1) {
                    item.set_enabled(wanted.1);
                }
                self.shown.insert(entry.id.clone(), wanted);
            }
            self.sync(&entry.submenu);
        }
    }

    /// Attach the menu to the OS window.
    pub fn attach(&self, window: &tao::window::Window) {
        #[cfg(target_os = "windows")]
        {
            use tao::platform::windows::WindowExtWindows;
            if let Err(error) = unsafe { self.menu.init_for_hwnd(window.hwnd() as _) } {
                tracing::warn!(%error, "could not attach the native menu");
            }
        }

        #[cfg(target_os = "macos")]
        {
            self.menu.init_for_nsapp();
            let _ = window;
        }

        // GTK: a menu bar packed above the web view in the window's box.
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            use tao::platform::unix::WindowExtUnix;
            if let Err(error) = self.menu.init_for_gtk_window(window.gtk_window(), window.default_vbox()) {
                tracing::warn!(%error, "could not attach the native menu");
            }
        }
    }
}
