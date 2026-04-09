//! Pure-ish ops over [`BrowserState`]: each op applies one [`Request`].
//!
//! The UI calls these on its own state, then performs the resulting engine
//! side-effects (webview create/navigate/close). Keeping ops engine-free
//! makes them testable without Chrome.

use crate::state::BrowserState;
use crate::Request;
use browser_layout::{scroll_step, Viewport};

/// Outcome of one op: what the caller must do to the engine/UI afterwards.
#[derive(Debug, Default, Clone)]
pub struct Effects {
    /// Pages that need a webview spawned (id, url).
    pub spawn: Vec<(u64, String)>,
    /// Pages that need a navigate (id, url).
    pub navigate: Vec<(u64, String)>,
    /// Pages whose reload must bypass the cache.
    pub hard_reload: Vec<u64>,
    /// Pages to reload in place (keeps history).
    pub soft_reload: Vec<u64>,
    /// Pages whose webview must be closed.
    pub close: Vec<u64>,
    /// Close the whole application.
    pub quit: bool,
    /// Open the URL prompt with this prefill.
    pub prompt_open: Option<String>,
    /// Open the command palette.
    pub palette_open: bool,
    /// Toast text.
    pub toast: Option<String>,
    /// The strip must re-center on the active page (focus changed, page
    /// added/closed/moved, workspace switched). Overlay-only requests (toast,
    /// prompt, palette) leave the scroll alone.
    pub scroll_recenter: bool,
}

impl From<()> for Effects {
    fn from(_: ()) -> Self {
        Self::default()
    }
}

/// Apply a request. `submit_search` hands a non-URL prompt text back to the
/// UI, which owns the search-engine config.
pub fn apply(state: &mut BrowserState, vp: &Viewport, req: Request, effects: &mut Effects) {
    match req {
        Request::Navigate(url) => {
            if let Some(id) = state.active_id() {
                state.set_url(id, &url);
                if let Some(slot) = state.slot_mut(id) {
                    slot.loading = true;
                }
                effects.navigate.push((id, url));
            } else {
                let id = state.add_page(&url, vp);
                effects.spawn.push((id, url));
            }
        }
        // Edit-the-current-URL: prefill the prompt like a real address bar.
        Request::FocusUrl => {
            let url = state
                .active_id()
                .and_then(|id| state.strip.page(id))
                .map(|p| p.url.clone())
                .unwrap_or_default();
            effects.prompt_open = Some(url);
        }
        Request::Reload => {
            if let Some(id) = state.active_id() {
                let has_url = state.strip.page(id).map(|p| !p.url.is_empty()).unwrap_or(false);
                if has_url {
                    effects.soft_reload.push(id);
                }
            }
        }
        // Bypass-cache reload re-navigates; the UI passes the hard flag on.
        Request::ReloadBypassCache => {
            if let Some(id) = state.active_id() {
                let has_url = state.strip.page(id).map(|p| !p.url.is_empty()).unwrap_or(false);
                if has_url {
                    effects.hard_reload.push(id);
                }
            }
        }
        Request::Back | Request::Forward => {
            // Engine-side history: the webview applies the intent; the url
            // event that follows updates state.
            effects.toast = None;
        }
        Request::PageNew | Request::PageNewBeside => {
            let id = state.add_page("", vp);
            effects.spawn.push((id, String::new()));
            effects.prompt_open = Some(String::new());
            effects.scroll_recenter = true;
        }
        Request::PageClose => {
            if let Some(id) = state.active_id() {
                state.close_page(id, vp);
                effects.close.push(id);
                effects.scroll_recenter = true;
            }
        }
        Request::FocusLeft | Request::PagePrev => {
            focus_neighbor(state, vp, false);
            effects.scroll_recenter = true;
        }
        Request::FocusRight | Request::PageNext => {
            focus_neighbor(state, vp, true);
            effects.scroll_recenter = true;
        }
        Request::FocusUp => {
            if let Some(ws) = state.next_workspace(false) {
                state.focus_workspace(ws, vp);
                effects.scroll_recenter = true;
            }
        }
        Request::FocusDown => {
            if let Some(ws) = state.next_workspace(true) {
                state.focus_workspace(ws, vp);
                effects.scroll_recenter = true;
            }
        }
        Request::PageMoveLeft | Request::PageMoveRight => {
            move_active(state, vp, matches!(req, Request::PageMoveRight));
            effects.scroll_recenter = true;
        }
        Request::WorkspaceNew => {
            let n = state.strip.workspaces.len() + 1;
            state.strip.create_workspace(format!("workspace {n}"));
        }
        Request::WorkspaceNext => {
            if let Some(ws) = state.next_workspace(true) {
                state.focus_workspace(ws, vp);
                effects.scroll_recenter = true;
            }
        }
        Request::WorkspacePrev => {
            if let Some(ws) = state.next_workspace(false) {
                state.focus_workspace(ws, vp);
                effects.scroll_recenter = true;
            }
        }
        Request::WorkspaceFocus(n) => {
            state.focus_workspace(n as u64, vp);
            effects.scroll_recenter = true;
        }
        Request::PageToWorkspace(n) => {
            if let Some(id) = state.active_id() {
                if let Some(p) = state.strip.page_mut(id) {
                    p.workspace = n as u64;
                }
                let successor = state
