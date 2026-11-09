//! The application menu: a port of
//! `app/src/main-process/menu/build-default-menu.ts`.
//!
//! The menu is plain data here. The shell builds a native menu from it and
//! asks [`action`] what a clicked item does; the renderer enables and
//! disables items by id through `update-menu-state`, exactly as it did with
//! Electron's menu.
//!
//! Labels use the Windows/Linux wording (with `&` mnemonics). The macOS
//! application menu and its title-case labels are not ported yet.

use serde::Deserialize;

use crate::ipc::protocol::SerializableMenuItem;

/// `MenuLabelsEvent`: renderer state that changes the wording of some items.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MenuLabels {
    pub selected_shell: Option<String>,
    pub selected_external_editor: Option<String>,
    pub ask_for_confirmation_on_force_push: bool,
    pub ask_for_confirmation_on_repository_removal: bool,
    pub has_current_pull_request: bool,
    pub contribution_target_default_branch: Option<String>,
    pub is_force_push_for_current_repository: bool,
    pub is_stashed_changes_visible: bool,
    pub ask_for_confirmation_when_stashing_all_changes: bool,
    pub is_changes_filter_visible: bool,
}

impl Default for MenuLabels {
    fn default() -> Self {
        Self {
            selected_shell: None,
            selected_external_editor: None,
            ask_for_confirmation_on_force_push: false,
            ask_for_confirmation_on_repository_removal: false,
            has_current_pull_request: false,
            contribution_target_default_branch: None,
            is_force_push_for_current_repository: false,
            is_stashed_changes_visible: false,
            ask_for_confirmation_when_stashing_all_changes: true,
            is_changes_filter_visible: true,
        }
    }
}

/// Text editing commands the Edit menu forwards to the web view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Reset,
    In,
    Out,
}

/// What clicking a menu item does.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    /// Send this `MenuEvent` to the renderer.
    Emit(&'static str),
    Edit(EditCommand),
    Zoom(Zoom),
    ToggleFullScreen,
    Reload,
    ToggleDevTools,
    OpenUrl(String),
    ShowLogs,
    Quit,
}

const ISSUES_URL: &str = "https://github.com/desktop/desktop/issues/new/choose";
const USER_GUIDES_URL: &str = "https://docs.github.com/en/desktop";
const SHORTCUTS_URL: &str = "https://docs.github.com/en/desktop/installing-and-configuring-github-desktop/overview/keyboard-shortcuts";

/// The action behind a menu item id, or `None` for an unknown id.
pub fn action(id: &str, labels: &MenuLabels) -> Option<MenuAction> {
    use MenuAction::*;
    Some(match id {
        // Items whose id differs from the event they emit.
        "new-repository" => Emit("create-repository"),
        "preferences" => Emit("show-preferences"),
        "find" => Emit("find-text"),
        "edit-select-all" => Emit("select-all"),
        "show-repository-list" => Emit("choose-repository"),
        "show-branches-list" => Emit("show-branches"),
        "show-worktrees-list" => Emit("show-worktrees"),
        "about" => Emit("show-about"),
        "create-pull-request" => Emit("open-pull-request"),
        "push" if labels.is_force_push_for_current_repository => Emit("force-push"),
        "toggle-stashed-changes" if labels.is_stashed_changes_visible => Emit("hide-stashed-changes"),
        "toggle-stashed-changes" => Emit("show-stashed-changes"),

        "edit-undo" => Edit(EditCommand::Undo),
        "edit-redo" => Edit(EditCommand::Redo),
        "edit-cut" => Edit(EditCommand::Cut),
        "edit-copy" => Edit(EditCommand::Copy),
        "edit-paste" => Edit(EditCommand::Paste),
        "zoom-reset" => Zoom(self::Zoom::Reset),
        "zoom-in" => Zoom(self::Zoom::In),
        "zoom-out" => Zoom(self::Zoom::Out),
        "toggle-full-screen" => ToggleFullScreen,
        "reload-window" => Reload,
        "show-devtools" => ToggleDevTools,
        "quit" => Quit,
        "help-report-issue" => OpenUrl(ISSUES_URL.into()),
        "help-contact-support" => OpenUrl(format!(
            "https://github.com/contact?from_desktop_app=1&app_version={}",
            env!("CARGO_PKG_VERSION")
        )),
        "help-user-guides" => OpenUrl(USER_GUIDES_URL.into()),
        "help-keyboard-shortcuts" => OpenUrl(SHORTCUTS_URL.into()),
        "help-show-logs" => ShowLogs,

        // Everything else emits an event named like its id.
        other => Emit(SAME_NAME_EVENTS.iter().copied().find(|event| *event == other)?),
    })
}

/// Items whose id is also the `MenuEvent` they emit.
const SAME_NAME_EVENTS: &[&str] = &[
    "add-local-repository",
    "clone-repository",
    "show-changes",
    "show-history",
    "go-to-commit-message",
    "toggle-changes-filter",
    "increase-active-resizable-width",
    "decrease-active-resizable-width",
    "push",
    "pull",
    "fetch",
    "remove-repository",
    "view-repository-on-github",
    "open-in-shell",
    "open-working-directory",
    "open-external-editor",
    "open-with-external-editor",
    "create-issue-in-repository-on-github",
    "create-worktree",
    "show-repository-settings",
    "create-branch",
    "rename-branch",
    "delete-branch",
    "discard-all-changes",
    "stash-all-changes",
    "update-branch-with-contribution-target-branch",
    "compare-to-branch",
    "merge-branch",
    "squash-and-merge-branch",
    "rebase-branch",
    "compare-on-github",
    "branch-on-github",
    "preview-pull-request",
];

/// The zoom steps Desktop supports, sorted.
const ZOOM_FACTORS: &[f64] = &[0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The zoom factor after stepping from `current` in `direction`.
pub fn next_zoom(current: f64, direction: Zoom) -> f64 {
    let closest = ZOOM_FACTORS
        .iter()
        .copied()
        .min_by(|a, b| (a - current).abs().total_cmp(&(b - current).abs()))
        .unwrap_or(1.0);
    match direction {
        Zoom::Reset => 1.0,
        Zoom::In => ZOOM_FACTORS.iter().copied().find(|f| *f > closest).unwrap_or(closest),
        Zoom::Out => ZOOM_FACTORS.iter().rev().copied().find(|f| *f < closest).unwrap_or(closest),
    }
}

fn item(id: &str, label: impl Into<String>, accelerator: Option<&str>) -> SerializableMenuItem {
    SerializableMenuItem {
        id: id.to_string(),
        label: label.into(),
        accelerator: accelerator.map(str::to_string),
        access_key: None,
        enabled: true,
        checked: false,
        visible: true,
        role: None,
        kind: "menuItem".into(),
        submenu: Vec::new(),
    }
}

fn separator(id: &str) -> SerializableMenuItem {
    SerializableMenuItem {
        kind: "separator".into(),
        ..item(id, "", None)
    }
}

fn submenu(id: &str, label: &str, items: Vec<SerializableMenuItem>) -> SerializableMenuItem {
    SerializableMenuItem {
        kind: "submenuItem".into(),
        submenu: items,
        ..item(id, label, None)
    }
}

fn truncate_with_ellipsis(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

/// Build the menu. Every item starts enabled; the renderer disables what
/// does not apply to its current state.
pub fn build(labels: &MenuLabels) -> Vec<SerializableMenuItem> {
    let file = submenu(
        "file",
        "&File",
        vec![
            item("new-repository", "New &repository…", Some("CmdOrCtrl+N")),
            separator("file-separator-1"),
            item("add-local-repository", "Add &local repository…", Some("CmdOrCtrl+O")),
            item("clone-repository", "Clo&ne repository…", Some("CmdOrCtrl+Shift+O")),
            separator("file-separator-2"),
            item("preferences", "&Options…", Some("CmdOrCtrl+,")),
            separator("file-separator-3"),
            item("quit", "E&xit", Some(if cfg!(windows) { "Alt+F4" } else { "CmdOrCtrl+Q" })),
        ],
    );

    // Undo/redo/cut/copy/paste carry no accelerator: the web view already
    // handles those keys, and a menu accelerator would run them twice.
    let edit = submenu(
        "edit",
        "&Edit",
        vec![
            item("edit-undo", "&Undo", None),
            item("edit-redo", "&Redo", None),
            separator("edit-separator-1"),
            item("edit-cut", "Cu&t", None),
            item("edit-copy", "&Copy", None),
            item("edit-paste", "&Paste", None),
            item("edit-select-all", "Select &all", Some("CmdOrCtrl+A")),
            separator("edit-separator-2"),
            item("find", "&Find", Some("CmdOrCtrl+F")),
        ],
    );

    let mut view = vec![
        item("show-changes", "&Changes", Some("CmdOrCtrl+1")),
        item("show-history", "&History", Some("CmdOrCtrl+2")),
        item("show-repository-list", "Repository &list", Some("CmdOrCtrl+T")),
        item("show-branches-list", "&Branches list", Some("CmdOrCtrl+B")),
        item("show-worktrees-list", "Wor&ktrees list", Some("CmdOrCtrl+Alt+W")),
        separator("view-separator-1"),
        item("go-to-commit-message", "Go to &Summary", Some("CmdOrCtrl+G")),
        item(
            "toggle-stashed-changes",
            if labels.is_stashed_changes_visible { "H&ide stashed changes" } else { "Sho&w stashed changes" },
            Some("Ctrl+H"),
        ),
        item(
            "toggle-changes-filter",
            format!(
                "{} Toggle Chan&ges Filter",
                if labels.is_changes_filter_visible { "Hide" } else { "Show" }
            ),
            Some("CmdOrCtrl+L"),
        ),
        item("toggle-full-screen", "Toggle &full screen", Some("F11")),
        separator("view-separator-2"),
        item("zoom-reset", "Reset zoom", Some("CmdOrCtrl+0")),
        item("zoom-in", "Zoom in", Some("CmdOrCtrl+=")),
        item("zoom-out", "Zoom out", Some("CmdOrCtrl+-")),
        item("increase-active-resizable-width", "Expand active resizable", Some("CmdOrCtrl+9")),
        item("decrease-active-resizable-width", "Contract active resizable", Some("CmdOrCtrl+8")),
        separator("view-separator-3"),
    ];
    // Electron showed Reload only on the development channel.
    if cfg!(debug_assertions) {
        view.push(item("reload-window", "&Reload", Some("CmdOrCtrl+Alt+R")));
    }
    view.push(item("show-devtools", "&Toggle developer tools", Some("Ctrl+Shift+I")));
    let view = submenu("view", "&View", view);

    let push_label = match (
        labels.is_force_push_for_current_repository,
        labels.ask_for_confirmation_on_force_push,
    ) {
        (false, _) => "P&ush",
        (true, true) => "Force P&ush…",
        (true, false) => "Force P&ush",
    };
    let file_manager = if cfg!(windows) { "Show in E&xplorer" } else { "Show in your File Manager" };

    let repository = submenu(
        "repository",
        "&Repository",
        vec![
            item("push", push_label, Some("CmdOrCtrl+P")),
            item("pull", "Pu&ll", Some("CmdOrCtrl+Shift+P")),
            item("fetch", "&Fetch", Some("CmdOrCtrl+Shift+T")),
            item(
                "remove-repository",
                if labels.ask_for_confirmation_on_repository_removal { "&Remove…" } else { "&Remove" },
                Some("CmdOrCtrl+Backspace"),
            ),
            separator("repository-separator-1"),
            item("view-repository-on-github", "&View on GitHub", Some("CmdOrCtrl+Shift+G")),
            item(
                "open-in-shell",
                format!("O&pen in {}", labels.selected_shell.as_deref().unwrap_or("shell")),
                Some("Ctrl+`"),
            ),
            item("open-working-directory", file_manager, Some("CmdOrCtrl+Shift+F")),
            item(
                "open-external-editor",
                format!(
                    "&Open in {}",
                    labels.selected_external_editor.as_deref().unwrap_or("external editor")
                ),
                Some("CmdOrCtrl+Shift+A"),
            ),
            item("open-with-external-editor", "Open &with…", Some("CmdOrCtrl+Shift+Alt+A")),
            separator("repository-separator-2"),
            item(
                "create-issue-in-repository-on-github",
                "Create &issue on GitHub",
                Some("CmdOrCtrl+I"),
            ),
            separator("repository-separator-3"),
            item("create-worktree", "New work&tree…", Some("CmdOrCtrl+Shift+W")),
            separator("repository-separator-4"),
            item("show-repository-settings", "Repository &settings…", None),
        ],
    );

    let default_branch = truncate_with_ellipsis(
        labels.contribution_target_default_branch.as_deref().unwrap_or("default branch"),
        25,
    );
    let branch = submenu(
        "branch",
        "&Branch",
        vec![
            item("create-branch", "New &branch…", Some("CmdOrCtrl+Shift+N")),
            item("rename-branch", "&Rename…", Some("CmdOrCtrl+Shift+R")),
            item("delete-branch", "&Delete…", Some("CmdOrCtrl+Shift+D")),
            separator("branch-separator-1"),
            item("discard-all-changes", "Discard all changes…", Some("CmdOrCtrl+Shift+Backspace")),
            item(
                "stash-all-changes",
                if labels.ask_for_confirmation_when_stashing_all_changes {
                    "&Stash all changes…"
                } else {
                    "&Stash all changes"
                },
                Some("CmdOrCtrl+Shift+S"),
            ),
            separator("branch-separator-2"),
            item(
                "update-branch-with-contribution-target-branch",
                format!("&Update from {default_branch}"),
                Some("CmdOrCtrl+Shift+U"),
            ),
            item("compare-to-branch", "&Compare to branch", Some("CmdOrCtrl+Shift+B")),
            item("merge-branch", "&Merge into current branch…", Some("CmdOrCtrl+Shift+M")),
            item(
                "squash-and-merge-branch",
                "Squas&h and merge into current branch…",
                Some("CmdOrCtrl+Shift+H"),
            ),
            item("rebase-branch", "R&ebase current branch…", Some("CmdOrCtrl+Shift+E")),
            separator("branch-separator-3"),
            item("compare-on-github", "Compare on &GitHub", Some("CmdOrCtrl+Shift+C")),
            item("branch-on-github", "View branch on GitHub", Some("CmdOrCtrl+Alt+B")),
            item("preview-pull-request", "Preview pull request", Some("CmdOrCtrl+Alt+P")),
            item(
                "create-pull-request",
                if labels.has_current_pull_request {
                    "View &pull request on GitHub"
                } else {
                    "Create &pull request"
                },
                Some("CmdOrCtrl+R"),
            ),
        ],
    );

    let show_logs = if cfg!(windows) {
        "S&how logs in Explorer"
    } else {
        "S&how logs in your File Manager"
    };
    let help = submenu(
        "help",
        "&Help",
        vec![
            item("help-report-issue", "Report issue…", None),
            item("help-contact-support", "&Contact GitHub support…", None),
            item("help-user-guides", "Show User Guides", None),
            item("help-keyboard-shortcuts", "Show keyboard shortcuts", None),
            item("help-show-logs", show_logs, None),
            separator("help-separator-1"),
            item("about", "&About GitHub Desktop", None),
        ],
    );

    vec![file, edit, view, repository, branch, help]
}

/// Carry the enabled/checked state of `current` over to a rebuilt menu, so
/// relabelling never re-enables an item the renderer had disabled.
pub fn merge_state(rebuilt: &mut [SerializableMenuItem], current: &[SerializableMenuItem]) {
    fn find<'a>(items: &'a [SerializableMenuItem], id: &str) -> Option<&'a SerializableMenuItem> {
        items.iter().find_map(|item| {
            if item.id == id {
                Some(item)
            } else {
                find(&item.submenu, id)
            }
        })
    }
    for item in rebuilt {
        if let Some(existing) = find(current, &item.id) {
            item.enabled = existing.enabled;
            item.checked = existing.checked;
        }
        merge_state(&mut item.submenu, current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn leaves(items: &[SerializableMenuItem], out: &mut Vec<SerializableMenuItem>) {
        for item in items {
            if item.submenu.is_empty() {
                out.push(item.clone());
            } else {
                leaves(&item.submenu, out);
            }
        }
    }

    #[test]
    fn every_item_has_a_unique_id_and_an_action() {
        let menu = build(&MenuLabels::default());
        assert_eq!(
            menu.iter().map(|m| m.label.clone()).collect::<Vec<_>>(),
            ["&File", "&Edit", "&View", "&Repository", "&Branch", "&Help"]
        );

        let mut items = Vec::new();
        leaves(&menu, &mut items);
        let mut seen = HashSet::new();
        for item in &items {
            assert!(seen.insert(item.id.clone()), "duplicate id {}", item.id);
            if item.kind == "separator" {
                continue;
            }
            assert!(
                action(&item.id, &MenuLabels::default()).is_some(),
                "{} does nothing when clicked",
                item.id
            );
        }
        assert_eq!(action("not-a-menu-item", &MenuLabels::default()), None);
    }

    #[test]
    fn ids_the_renderer_toggles_exist() {
        // A sample of the ids in app/src/lib/menu-update.ts.
        let menu = build(&MenuLabels::default());
        let mut items = Vec::new();
        leaves(&menu, &mut items);
        let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
        for id in [
            "push", "pull", "fetch", "create-branch", "rename-branch", "delete-branch",
            "show-changes", "show-history", "show-repository-list", "show-branches-list",
            "open-working-directory", "show-repository-settings", "go-to-commit-message",
            "create-pull-request", "compare-to-branch", "toggle-stashed-changes",
            "clone-repository", "new-repository", "add-local-repository", "preferences", "about",
        ] {
            assert!(ids.contains(id), "missing {id}");
        }
    }

    #[test]
    fn labels_and_actions_follow_renderer_state() {
        let labels: MenuLabels = serde_json::from_value(serde_json::json!({
            "selectedShell": "Konsole",
            "selectedExternalEditor": "Zed",
            "askForConfirmationOnForcePush": true,
            "askForConfirmationOnRepositoryRemoval": true,
            "isForcePushForCurrentRepository": true,
            "hasCurrentPullRequest": true,
            "isStashedChangesVisible": true,
            "contributionTargetDefaultBranch": "a-very-long-default-branch-name-indeed",
        }))
        .unwrap();
        let mut items = Vec::new();
        leaves(&build(&labels), &mut items);
        let label = |id: &str| items.iter().find(|i| i.id == id).unwrap().label.clone();

        assert_eq!(label("push"), "Force P&ush…");
        assert_eq!(label("open-in-shell"), "O&pen in Konsole");
        assert_eq!(label("open-external-editor"), "&Open in Zed");
        assert_eq!(label("remove-repository"), "&Remove…");
        assert_eq!(label("create-pull-request"), "View &pull request on GitHub");
        assert_eq!(label("toggle-stashed-changes"), "H&ide stashed changes");
        assert_eq!(
            label("update-branch-with-contribution-target-branch"),
            "&Update from a-very-long-default-bran…"
        );
        // Unspecified fields keep their defaults.
        assert_eq!(label("stash-all-changes"), "&Stash all changes…");

        assert_eq!(action("push", &labels), Some(MenuAction::Emit("force-push")));
        assert_eq!(action("push", &MenuLabels::default()), Some(MenuAction::Emit("push")));
        assert_eq!(
            action("toggle-stashed-changes", &labels),
            Some(MenuAction::Emit("hide-stashed-changes"))
        );
    }

    #[test]
    fn relabelling_keeps_disabled_items_disabled() {
        let mut current = build(&MenuLabels::default());
        current[3].submenu[0].enabled = false; // Repository > Push

        let mut rebuilt = build(&MenuLabels {
            is_force_push_for_current_repository: true,
            ..MenuLabels::default()
        });
        merge_state(&mut rebuilt, &current);

        assert_eq!(rebuilt[3].submenu[0].label, "Force P&ush");
        assert!(!rebuilt[3].submenu[0].enabled);
        assert!(rebuilt[3].submenu[1].enabled);
    }

    #[test]
    fn zoom_steps_through_the_supported_factors() {
        assert_eq!(next_zoom(1.0, Zoom::In), 1.1);
        assert_eq!(next_zoom(1.0, Zoom::Out), 0.9);
        assert_eq!(next_zoom(1.07, Zoom::In), 1.25);
        assert_eq!(next_zoom(2.0, Zoom::In), 2.0);
        assert_eq!(next_zoom(0.67, Zoom::Out), 0.67);
        assert_eq!(next_zoom(1.75, Zoom::Reset), 1.0);
    }
}
