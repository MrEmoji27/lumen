//! Named sets of tabs. Each workspace is a list of saved tabs and a name.
//! Only the active workspace's tabs live in memory as Tab objects; hidden
//! workspaces hold their tabs as serialized state and cost nothing.

use crate::store::{SavedTab, Session, Workspace};

/// The workspace a fresh install starts on.
pub const DEFAULT_NAME: &str = "Main";

/// Ctrl+Alt+1..4 covers these. Four is plenty.
pub const MAX_WORKSPACES: usize = 4;

/// Migrate a session file from the pre-workspaces era (a flat tab list and an
/// active index) into the workspace list. Old files have an empty `workspaces`
/// vec; this turns them into a single "Main" workspace and returns it.
pub fn from_session(session: &Session) -> Vec<Workspace> {
    if !session.workspaces.is_empty() {
        return session.workspaces.clone();
    }
    let active = session.active.min(session.tabs.len().saturating_sub(1));
    vec![Workspace {
        name: DEFAULT_NAME.to_string(),
        tabs: session.tabs.clone(),
        active,
        colour: String::new(),
    }]
}

/// Build the initial workspace list from a session. If the session predates
/// workspaces, the tabs become one "Main" workspace.
pub fn build_from_session(session: &Session) -> (Vec<WorkspaceData>, usize) {
    let list: Vec<WorkspaceData> = from_session(session)
        .iter()
        .map(WorkspaceData::from)
        .collect();
    let list = if list.is_empty() {
        vec![WorkspaceData::new(DEFAULT_NAME)]
    } else {
        list
    };
    let active = session
        .active_workspace
        .min(list.len().saturating_sub(1));
    (list, active)
}

/// Build a session for saving, stamping the current workspace's tabs and
/// active index back into it. `active_tabs` is the serialized form of the
/// currently-loaded Tab objects; `active_tab_idx` points at the active one.
/// The `tabs`/`active` fields mirror the active workspace so a pre-workspaces
/// build can still read the file.
pub fn to_session(
    workspaces: &[WorkspaceData],
    active_ws_idx: usize,
    active_tabs: &[SavedTab],
    active_tab_idx: usize,
    width: f64,
    height: f64,
    maximized: bool,
) -> Session {
    let a = active_ws_idx.min(workspaces.len().saturating_sub(1));
    let ws_list: Vec<Workspace> = workspaces
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i == a {
                Workspace {
                    name: w.name.clone(),
                    tabs: active_tabs.to_vec(),
                    active: active_tab_idx.min(active_tabs.len().saturating_sub(1)),
                    colour: w.colour.clone(),
                }
            } else {
                w.to_workspace()
            }
        })
        .collect();
    let tabs = ws_list.get(a).map(|w| w.tabs.clone()).unwrap_or_default();
    let active = ws_list.get(a).map(|w| w.active).unwrap_or(0);
    Session {
        tabs,
        active,
        width,
        height,
        maximized,
        workspaces: ws_list,
        active_workspace: a,
        essentials: Vec::new(),
    }
}

/// Runtime state of one workspace. `tab_ids` is only valid for the active
/// workspace; inactive workspaces store their tabs as `saved`.
#[derive(Clone, Debug)]
pub struct WorkspaceData {
    pub name: String,
    /// Tab IDs currently in memory. Only the active workspace populates this.
    pub tab_ids: Vec<u64>,
    /// Serialized tabs, kept for inactive workspaces (cheap, no WebView memory).
    pub saved: Vec<SavedTab>,
    /// Index of the active tab within `tab_ids` (active) or `saved` (inactive).
    pub active: usize,
    /// The colour this workspace wears: a number in the palette, empty for none.
    pub colour: String,
}

impl WorkspaceData {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            tab_ids: Vec::new(),
            saved: Vec::new(),
            active: 0,
            colour: String::new(),
        }
    }

    /// The persisted Workspace view of this inactive workspace.
    pub fn to_workspace(&self) -> Workspace {
        Workspace {
            name: self.name.clone(),
            tabs: self.saved.clone(),
            active: self.active.min(self.saved.len().saturating_sub(1)),
            colour: self.colour.clone(),
        }
    }

    /// True when this workspace currently has its tabs in memory.
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        !self.tab_ids.is_empty()
    }

    /// Tab IDs that should be discarded when leaving this workspace (all of them).
    #[allow(dead_code)]
    pub fn all_ids(&self) -> Vec<u64> {
        self.tab_ids.clone()
    }

    /// The active tab ID, if this workspace is the active one.
    #[allow(dead_code)]
    pub fn active_id(&self) -> Option<u64> {
        self.tab_ids.get(self.active).copied()
    }
}

impl From<&Workspace> for WorkspaceData {
    fn from(ws: &Workspace) -> Self {
        WorkspaceData {
            name: ws.name.clone(),
            tab_ids: Vec::new(),
            saved: ws.tabs.clone(),
            active: ws.active.min(ws.tabs.len().saturating_sub(1)),
            colour: ws.colour.clone(),
        }
    }
}

/// Switch from one workspace to another. Returns the tab IDs to discard (the
/// old workspace's) and the tab ID to activate (the new workspace's active tab).
#[allow(dead_code)]
pub fn switch(
    workspaces: &[WorkspaceData],
    from: usize,
    to: usize,
) -> (Vec<u64>, Option<u64>) {
    let discard = workspaces.get(from).map(|w| w.all_ids()).unwrap_or_default();
    let activate = workspaces.get(to).and_then(|w| w.active_id());
    (discard, activate)
}

/// Rename a workspace in place.
#[allow(dead_code)]
pub fn rename(workspaces: &mut [WorkspaceData], idx: usize, name: &str) {
    if let Some(ws) = workspaces.get_mut(idx) {
        ws.name = name.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SavedTab, Session};

    fn tab(url: &str, title: &str, pinned: bool) -> SavedTab {
        SavedTab {
            url: url.to_string(),
            title: title.to_string(),
            favicon: String::new(),
            pinned,
            pin_url: if pinned { url.to_string() } else { String::new() },
            essential: false,
            idle_ms: 0,
        }
    }

    // ------------------------------------------------------------------ format

    /// An old session.json with no `workspaces` field loads as one "Main".
    #[test]
    fn old_session_becomes_main() {
        let session = Session {
            tabs: vec![tab("https://a.com", "A", false), tab("https://b.com", "B", false)],
            active: 1,
            width: 1024.0,
            height: 768.0,
            maximized: false,
            workspaces: Vec::new(),
            active_workspace: 0,
            essentials: Vec::new(),
            archived: Vec::new(),
        };
        let (ws, active) = build_from_session(&session);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].name, "Main");
        assert_eq!(ws[0].saved.len(), 2);
        assert_eq!(active, 0);
    }

    /// An old session with a single tab still works.
    #[test]
    fn old_session_single_tab() {
        let session = Session {
            tabs: vec![tab("https://x.com", "X", false)],
            active: 0,
            ..Default::default()
        };
        let (ws, active) = build_from_session(&session);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].saved.len(), 1);
        assert_eq!(ws[0].active, 0);
        assert_eq!(active, 0);
    }

    /// A new session with workspaces loads them all.
    #[test]
    fn new_session_with_workspaces() {
        let mut session = Session::default();
        session.workspaces = vec![
            Workspace { name: "Main".into(), tabs: vec![tab("https://a.com", "A", false)], active: 0, colour: String::new() },
            Workspace { name: "Work".into(), tabs: vec![tab("https://b.com", "B", false), tab("https://c.com", "C", false)], active: 1, colour: String::new() },
        ];
        session.active_workspace = 1;
        let (ws, active) = build_from_session(&session);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "Main");
        assert_eq!(ws[1].name, "Work");
        assert_eq!(ws[1].saved.len(), 2);
        assert_eq!(ws[1].active, 1);
        assert_eq!(active, 1);
    }

    /// An empty session (no tabs, no workspaces) still creates Main.
    #[test]
    fn empty_session_creates_main() {
        let session = Session::default();
        let (ws, active) = build_from_session(&session);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].name, "Main");
        assert_eq!(active, 0);
    }

    /// Round-trip: save workspaces then reload, nothing is lost.
    #[test]
    fn round_trip_preserves_workspaces() {
        let session = Session {
            workspaces: vec![
                Workspace { name: "Main".into(), tabs: vec![tab("https://a.com", "A", false)], active: 0, colour: String::new() },
                Workspace { name: "Work".into(), tabs: vec![tab("https://b.com", "B", false), tab("https://c.com", "C", false)], active: 1, colour: String::new() },
            ],
            active_workspace: 1,
            ..Default::default()
        };
        let (ws, active) = build_from_session(&session);
        let s = to_session(&ws, active, &ws[1].saved.clone(), ws[1].active, 800.0, 600.0, true);
        assert_eq!(s.workspaces.len(), 2);
        assert_eq!(s.workspaces[1].name, "Work");
        assert_eq!(s.workspaces[1].tabs.len(), 2);
        assert_eq!(s.workspaces[1].active, 1);
        assert_eq!(s.active_workspace, 1);
        assert_eq!(s.width, 800.0);
        assert_eq!(s.height, 600.0);
        assert!(s.maximized);
        // The active workspace's tabs also appear in the compat fields.
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.active, 1);
    }

    /// Pins belong to the workspace they were made in, and keep the page they were pinned on
    /// whether that workspace is the live one or a stored one.
    #[test]
    fn pins_stay_in_their_workspace() {
        let mut work_pin = tab("https://mail.work/inbox", "Mail", true);
        work_pin.pin_url = "https://mail.work/".into();
        let session = Session {
            workspaces: vec![
                Workspace { name: "Main".into(), tabs: vec![tab("https://music.example/", "Music", true)], active: 0, colour: String::new() },
                Workspace { name: "Work".into(), tabs: vec![work_pin], active: 0, colour: String::new() },
            ],
            active_workspace: 0,
            ..Default::default()
        };
        let (ws, active) = build_from_session(&session);
        let live = ws[0].saved.clone();
        let s = to_session(&ws, active, &live, 0, 800.0, 600.0, false);
        assert!(s.workspaces[0].tabs[0].pinned);
        assert_eq!(s.workspaces[0].tabs[0].pin_url, "https://music.example/");
        assert!(s.workspaces[1].tabs[0].pinned);
        assert_eq!(s.workspaces[1].tabs[0].pin_url, "https://mail.work/");
        // And it all survives the file.
        let back: Session = serde_json::from_str(&serde_json::to_string(&s).expect("to json")).expect("from json");
        assert_eq!(back.workspaces[1].tabs[0].pin_url, "https://mail.work/");
    }

    // ---------------------------------------------------------------- switching

    /// Switching discards the old workspace's tabs and activates the new one.
    #[test]
    fn switch_discards_and_activates() {
        let ws = vec![
            WorkspaceData { name: "Main".into(), tab_ids: vec![1, 2], saved: Vec::new(), active: 0, colour: String::new() },
            WorkspaceData { name: "Work".into(), tab_ids: vec![3, 4, 5], saved: Vec::new(), active: 1, colour: String::new() },
        ];
        let (discard, activate) = switch(&ws, 0, 1);
        assert_eq!(discard, vec![1, 2]);
        assert_eq!(activate, Some(4));
    }

    /// Switching to a workspace with no tabs activates nothing.
    #[test]
    fn switch_to_empty_workspace() {
        let ws = vec![
            WorkspaceData { name: "Main".into(), tab_ids: vec![1], saved: Vec::new(), active: 0, colour: String::new() },
            WorkspaceData::new("Empty"),
        ];
        let (discard, activate) = switch(&ws, 0, 1);
        assert_eq!(discard, vec![1]);
        assert_eq!(activate, None);
    }

    /// Out-of-range indices are handled gracefully.
    #[test]
    fn switch_out_of_range_is_safe() {
        let ws = vec![WorkspaceData::new("Main")];
        let (discard, activate) = switch(&ws, 0, 5);
        assert!(discard.is_empty());
        assert_eq!(activate, None);
    }

    /// `from_session` trusts existing workspaces when present.
    #[test]
    fn from_session_preserves_existing() {
        let session = Session {
            workspaces: vec![
                Workspace { name: "One".into(), tabs: vec![tab("https://a.com", "A", false)], active: 0, colour: String::new() },
                Workspace { name: "Two".into(), tabs: vec![], active: 0, colour: String::new() },
            ],
            active_workspace: 1,
            ..Default::default()
        };
        let ws = from_session(&session);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "One");
        assert_eq!(ws[1].name, "Two");
    }

    /// `active_id` returns the active tab for an active workspace.
    #[test]
    fn active_id_points_at_active_tab() {
        let ws = WorkspaceData {
            name: "Main".into(),
            tab_ids: vec![10, 20, 30],
            saved: Vec::new(),
            active: 2,
            colour: String::new(),
        };
        assert_eq!(ws.active_id(), Some(30));
    }

    /// An inactive workspace has no tab IDs and no active ID.
    #[test]
    fn inactive_workspace_has_no_active_id() {
        let ws = WorkspaceData {
            name: "Main".into(),
            tab_ids: Vec::new(),
            saved: vec![tab("https://a.com", "A", false)],
            active: 0,
            colour: String::new(),
        };
        assert!(!ws.is_active());
        assert_eq!(ws.active_id(), None);
    }

    /// Renaming a workspace works.
    #[test]
    fn rename_changes_name() {
        let mut ws = vec![WorkspaceData::new("Old")];
        rename(&mut ws, 0, "New");
        assert_eq!(ws[0].name, "New");
        rename(&mut ws, 5, "Ignored");
        assert_eq!(ws[0].name, "New");
    }

    /// `to_workspace` serializes the saved-tab list, not the live tab IDs.
    #[test]
    fn to_workspace_emits_saved() {
        let ws = WorkspaceData {
            name: "Main".into(),
            tab_ids: vec![1, 2],
            saved: vec![tab("https://a.com", "A", false)],
            active: 0,
            colour: String::new(),
        };
        let w = ws.to_workspace();
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.name, "Main");
    }
}
