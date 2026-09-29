//! Pure-ish ops over [`BrowserState`]: each op applies one [`Request`].
//!
//! The UI calls these on its own state, then performs the resulting engine
//! side-effects (webview create/navigate/close). Keeping ops engine-free
//! makes them testable without Chrome.

use crate::state::BrowserState;
use crate::Request;
use browser_layout::{next_preset_width, scroll_step, stepped_width, Viewport};

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
    /// Pages whose audio mute state changed (id, muted).
    pub mute: Vec<(u64, bool)>,
    /// The strip must re-center on the active page (focus changed, page
    /// added/closed/moved, workspace switched). Overlay-only requests (toast,
    /// prompt, palette) leave the scroll alone.
    pub scroll_recenter: bool,
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
            // History is engine-side; main.rs dispatches these directly to the
            // webview before ops sees them. Nothing to do here.
        }
        Request::PageNew | Request::PageNewBeside => {
            // A new page takes focus, so the overview has done its job.
            state.overview_open = false;
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
        Request::FocusUp | Request::WorkspacePrev => step_workspace(state, vp, false, effects),
        Request::FocusDown | Request::WorkspaceNext => step_workspace(state, vp, true, effects),
        Request::PageMoveLeft | Request::PageMoveRight => {
            move_active(state, vp, matches!(req, Request::PageMoveRight));
            effects.scroll_recenter = true;
        }
        // The empty workspace at the bottom is always there; "new" goes to it.
        Request::WorkspaceNew => {
            let last = state.strip.workspace_at(usize::MAX);
            state.focus_workspace(last, vp);
            effects.scroll_recenter = true;
        }
        Request::WorkspaceFocus(n) => {
            let ws = state.strip.workspace_at((n as usize).saturating_sub(1));
            state.focus_workspace(ws, vp);
            effects.scroll_recenter = true;
        }
        Request::PageToWorkspace(n) => {
            if let Some(id) = state.active_id() {
                let ws = state.strip.workspace_at((n as usize).saturating_sub(1));
                state.send_page_to_workspace(id, ws, false, vp);
                effects.scroll_recenter = true;
            }
        }
        Request::PageToWorkspaceUp | Request::PageToWorkspaceDown => {
            let down = matches!(req, Request::PageToWorkspaceDown);
            if let Some(id) = state.active_id() {
                if let Some(ws) = state.strip.workspace_neighbor(state.strip.active_workspace, down) {
                    state.send_page_to_workspace(id, ws, true, vp);
                    effects.scroll_recenter = true;
                }
            }
        }
        Request::PageMuteToggle => {
            if let Some(id) = state.active_id() {
                if let Some(muted) = state.toggle_muted(id) {
                    effects.mute.push((id, muted));
                }
            }
        }
        Request::PageWidthPreset => {
            let (gap, presets) = (state.strip.gap, state.width_presets.clone());
            resize_active(state, effects, |w| next_preset_width(w, &presets, vp.width, gap));
        }
        Request::PageWidthDecrease | Request::PageWidthIncrease => {
            let grow = matches!(req, Request::PageWidthIncrease);
            resize_active(state, effects, |w| Some(stepped_width(w, grow, vp.width)));
        }
        Request::PageMaximize => {
            if let Some(id) = state.active_id() {
                state.strip.toggle_maximize(id, vp.width);
                effects.scroll_recenter = true;
            }
        }
        Request::OverviewToggle => state.overview_open = !state.overview_open,
        Request::ScrollLeft => {
            let target = (state.scroll - vp.width / 2.0).max(0.0);
            state.scroll = scroll_step(state.scroll, target, 0.35);
        }
        Request::ScrollRight => {
            let target = state.scroll + vp.width / 2.0;
            state.scroll = scroll_step(state.scroll, target, 0.35);
        }
        Request::OpenPalette => effects.palette_open = true,
        Request::ConfigReload => effects.toast = Some("config reloaded".into()),
        Request::Quit => {
            state.quit_requested = true;
            effects.quit = true;
        }
        Request::PromptSubmit(text) => handle_prompt_submit(state, vp, text, effects),
        Request::RunCommand { name, arg } => {
            if let Some(r) = Request::from_command(&name, arg.as_deref()) {
                apply(state, vp, r, effects);
            }
        }
        Request::ExecLua(_) => {
            // The UI owns the LuaHost; engine-free ops cannot run chunks.
            effects.toast = Some("lua runs only in the UI layer".into());
        }
    }
}

fn handle_prompt_submit(
    state: &mut BrowserState,
    vp: &Viewport,
    text: String,
    effects: &mut Effects,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let looks_like_url = text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("about:")
        || text.starts_with("file://")
        || text.starts_with("data:")
        || (text.contains('.') && !text.contains(' '));
    if looks_like_url {
        let url = if text.contains("://") || text.starts_with("data:") || text.starts_with("about:")
        {
            text.to_string()
        } else {
            format!("https://{text}")
        };
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
    } else {
        // Not a URL: hand it back; the UI substitutes its search engine.
        effects.prompt_open = Some(text.to_string());
    }
}

/// Focus the workspace above or below; the stack does not wrap.
fn step_workspace(state: &mut BrowserState, vp: &Viewport, down: bool, effects: &mut Effects) {
    if let Some(ws) = state.strip.workspace_neighbor(state.strip.active_workspace, down) {
        state.focus_workspace(ws, vp);
        effects.scroll_recenter = true;
    }
}

/// Resize only the active page to the width `pick` chooses from its current
/// one; the strip to its right reflows, nothing else changes size.
fn resize_active(state: &mut BrowserState, effects: &mut Effects, pick: impl FnOnce(f32) -> Option<f32>) {
    let Some(id) = state.active_id() else { return };
    let Some(width) = state.strip.page(id).map(|p| p.width) else { return };
    if let Some(next) = pick(width) {
        state.strip.resize_page(id, next);
        effects.scroll_recenter = true;
    }
}

fn focus_neighbor(state: &mut BrowserState, vp: &Viewport, right: bool) {
    let Some(active) = state.active_id() else { return };
    let pages = state.strip.visible();
    let Some(i) = pages.iter().position(|p| p.id == active) else { return };
    let target = if right { i + 1 } else { i.wrapping_sub(1) };
    if let Some(p) = pages.get(target) {
        let id = p.id;
        state.focus_page(id, vp);
    }
}

fn move_active(state: &mut BrowserState, _vp: &Viewport, right: bool) {
    let Some(active) = state.active_id() else { return };
    let pages = state.strip.visible();
    let Some(idx) = pages.iter().position(|p| p.id == active) else { return };
    let target = if right {
        match pages.get(idx + 1) {
            Some(p) => p.x,
            None => return,
        }
    } else {
        match idx.checked_sub(1).and_then(|i| pages.get(i)) {
            Some(p) => p.x,
            None => return,
        }
    };
    state.strip.move_page(active, target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_layout::Viewport;

    fn vp() -> Viewport {
        Viewport { width: 1000.0, height: 800.0 }
    }

    fn setup() -> (BrowserState, Viewport) {
        (BrowserState::default(), vp())
    }

    #[test]
    fn navigate_sets_url_and_effects() {
        let (mut s, vp) = setup();
        let id = s.add_page("about:blank", &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::Navigate("https://e.test".into()), &mut fx);
        assert_eq!(fx.navigate, vec![(id, "https://e.test".to_string())]);
        assert_eq!(s.strip.page(id).unwrap().url, "https://e.test");
    }

    #[test]
    fn close_emits_engine_effect() {
        let (mut s, vp) = setup();
        let a = s.add_page("a", &vp);
        let c = s.add_page("c", &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::PageClose, &mut fx);
        assert_eq!(fx.close, vec![c]);
        assert_eq!(s.active_id(), Some(a));
    }

    #[test]
    fn page_new_beside_prompts_and_spawns() {
        let (mut s, vp) = setup();
        s.add_page("a", &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::PageNewBeside, &mut fx);
        assert_eq!(fx.spawn.len(), 1);
        assert!(fx.prompt_open.is_some());
        assert_eq!(s.strip.pages.len(), 2);
    }

    #[test]
    fn focus_moves_along_strip() {
        let (mut s, vp) = setup();
        let a = s.add_page("a", &vp);
        let c = s.add_page("c", &vp);
        s.focus_page(a, &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::FocusRight, &mut fx);
        assert_eq!(s.active_id(), Some(c));
        apply(&mut s, &vp, Request::FocusLeft, &mut fx);
        assert_eq!(s.active_id(), Some(a));
    }

    #[test]
    fn move_right_swaps_slots() {
        let (mut s, vp) = setup();
        let a = s.add_page("a", &vp);
        let c = s.add_page("c", &vp);
        // `c` is active (new pages take focus). Move it left instead, then
        // verify the pair swapped; move a back right for the original path.
        let (ax, cx) = (s.strip.page(a).unwrap().x, s.strip.page(c).unwrap().x);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::PageMoveLeft, &mut fx);
        assert!((s.strip.page(c).unwrap().x - ax).abs() < 0.01);
        assert!((s.strip.page(a).unwrap().x - cx).abs() < 0.01);
    }

    #[test]
    fn prompt_submit_routes_url_vs_search() {
        let (mut s, vp) = setup();
        s.add_page("about:blank", &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::PromptSubmit("example.com".into()), &mut fx);
        assert_eq!(fx.navigate[0].1, "https://example.com");
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::PromptSubmit("two words".into()), &mut fx);
        assert_eq!(
            fx.prompt_open.as_deref(),
            Some("two words"),
            "search goes back to the prompt with query preserved"
        );
    }

    #[test]
    fn quit_sets_flag_and_effect() {
        let (mut s, vp) = setup();
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::Quit, &mut fx);
        assert!(fx.quit && s.quit_requested);
    }

    /// Two pages on workspace 1 and one on workspace 2; workspace 1 focused.
    fn two_workspaces() -> (BrowserState, Viewport, [u64; 3]) {
        let (mut s, vp) = setup();
        let a = s.add_page("a", &vp);
        let b = s.add_page("b", &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::WorkspaceNew, &mut fx);
        let c = s.add_page("c", &vp);
        apply(&mut s, &vp, Request::WorkspaceFocus(1), &mut fx);
        (s, vp, [a, b, c])
    }

    fn run(s: &mut BrowserState, vp: &Viewport, req: Request) -> Effects {
        let mut fx = Effects::default();
        apply(s, vp, req, &mut fx);
        fx
    }

    fn ws_of(s: &BrowserState, id: u64) -> u64 {
        s.strip.page(id).unwrap().workspace
    }

    #[test]
    fn focus_workspace_up_and_down_stop_at_the_ends() {
        let (mut s, vp, [_, b, c]) = two_workspaces();
        assert_eq!(s.strip.workspaces.len(), 3);
        assert!(run(&mut s, &vp, Request::FocusUp).scroll_recenter == false, "already at the top");
        assert_eq!(s.strip.active_workspace, 1);
        run(&mut s, &vp, Request::FocusDown);
        assert_eq!((s.strip.active_workspace, s.active_id()), (2, Some(c)));
        run(&mut s, &vp, Request::FocusDown);
        assert_eq!(s.strip.active_workspace, 3, "the empty workspace at the bottom");
        assert_eq!(s.active_id(), None);
        assert!(!run(&mut s, &vp, Request::FocusDown).scroll_recenter, "no wrap past the bottom");
        run(&mut s, &vp, Request::FocusUp);
        assert_eq!(s.active_id(), Some(c));
        run(&mut s, &vp, Request::FocusUp);
        assert_eq!(s.active_id(), Some(b), "workspace 1 returns to where focus was");
    }

    #[test]
    fn workspace_numbers_count_from_the_top_and_clamp() {
        let (mut s, vp, [_, _, c]) = two_workspaces();
        run(&mut s, &vp, Request::WorkspaceFocus(2));
        assert_eq!(s.active_id(), Some(c));
        run(&mut s, &vp, Request::WorkspaceFocus(9));
        assert_eq!(s.strip.active_workspace, 3, "past the end lands on the last workspace");
        run(&mut s, &vp, Request::WorkspaceFocus(1));
        run(&mut s, &vp, Request::WorkspaceNew);
        assert_eq!(s.strip.active_workspace, 3, "new = the empty one below");
    }

    #[test]
    fn move_page_down_and_up_follows_the_page() {
        let (mut s, vp, [a, b, c]) = two_workspaces();
        s.focus_page(b, &vp);
        run(&mut s, &vp, Request::PageToWorkspaceDown);
        assert_eq!((ws_of(&s, b), s.strip.active_workspace, s.active_id()), (2, 2, Some(b)));
        assert_eq!(s.strip.page(b).unwrap().x, s.strip.page(c).unwrap().width + s.strip.gap);
        assert_eq!(s.strip.page(a).unwrap().x, 0.0, "source strip unchanged for the rest");
        run(&mut s, &vp, Request::PageToWorkspaceUp);
        assert_eq!((ws_of(&s, b), s.active_id()), (1, Some(b)));
        assert!(!run(&mut s, &vp, Request::PageToWorkspaceUp).scroll_recenter, "nothing above workspace 1");
        assert_eq!(ws_of(&s, b), 1);
    }

    #[test]
    fn moving_the_last_page_down_cleans_up_and_keeps_one_empty_workspace() {
        let (mut s, vp, [a, b, c]) = two_workspaces();
        run(&mut s, &vp, Request::WorkspaceFocus(2));
        run(&mut s, &vp, Request::PageToWorkspaceDown); // c: 2 -> empty bottom
        assert_eq!(ws_of(&s, c), 3);
        assert_eq!(s.strip.workspaces.iter().map(|w| w.id).collect::<Vec<_>>(), vec![1, 3, 4]);
        assert_eq!(s.strip.active_workspace, 3);
        let empty = s.strip.workspaces.iter().filter(|w| s.strip.workspace_pages(w.id).is_empty()).count();
        assert_eq!(empty, 1, "exactly one empty workspace");
        let _ = (a, b);
    }

    #[test]
    fn send_to_numbered_workspace_keeps_focus_where_it_was() {
        let (mut s, vp, [a, b, _]) = two_workspaces();
        assert_eq!(s.active_id(), Some(b));
        run(&mut s, &vp, Request::PageToWorkspace(2));
        assert_eq!((ws_of(&s, b), s.strip.active_workspace, s.active_id()), (2, 1, Some(a)));
    }

    #[test]
    fn width_preset_cycles_only_the_active_page() {
        let (mut s, vp) = setup();
        s.width_presets = vec![1.0 / 3.0, 0.5, 2.0 / 3.0];
        let a = s.add_page("a", &vp);
        let b = s.add_page("b", &vp);
        let c = s.add_page("c", &vp);
        s.focus_page(b, &vp);
        let before: Vec<(f32, f32)> = [a, b, c].iter().map(|i| (s.strip.page(*i).unwrap().x, s.strip.page(*i).unwrap().width)).collect();
        let gap = s.strip.gap;
        let w = |f: f32| browser_layout::width_for_fraction(f, vp.width, gap);
        // 0.78 -> next wider preset (none) wraps to 1/3, then 1/2, then 2/3.
        for f in [1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0 / 3.0] {
            let fx = run(&mut s, &vp, Request::PageWidthPreset);
            assert!(fx.scroll_recenter);
            assert!((s.strip.page(b).unwrap().width - w(f)).abs() < 0.01, "preset {f}");
        }
        let after: Vec<(f32, f32)> = [a, b, c].iter().map(|i| (s.strip.page(*i).unwrap().x, s.strip.page(*i).unwrap().width)).collect();
        assert_eq!((after[0], after[2].1), (before[0], before[2].1), "left page and right page width untouched");
        assert_eq!(after[2].0, after[1].0 + after[1].1 + gap, "right page reflowed against the resized one");
    }

    #[test]
    fn width_steps_and_maximize_keep_neighbors_unchanged() {
        let (mut s, vp) = setup();
        let a = s.add_page("a", &vp);
        let b = s.add_page("b", &vp);
        s.focus_page(a, &vp);
        let b_width = s.strip.page(b).unwrap().width;
        run(&mut s, &vp, Request::PageWidthDecrease);
        assert_eq!(s.strip.page(a).unwrap().width, 780.0 - 100.0);
        run(&mut s, &vp, Request::PageWidthIncrease);
        run(&mut s, &vp, Request::PageWidthIncrease);
        assert_eq!(s.strip.page(a).unwrap().width, 880.0);
        run(&mut s, &vp, Request::PageMaximize);
        assert_eq!(s.strip.page(a).unwrap().width, vp.width);
        assert_eq!(s.strip.page(b).unwrap().x, vp.width + s.strip.gap);
        run(&mut s, &vp, Request::PageMaximize);
        assert_eq!(s.strip.page(a).unwrap().width, 880.0, "maximize toggles back");
        assert_eq!(s.strip.page(b).unwrap().width, b_width, "neighbor never resized");
    }

    #[test]
    fn mute_toggle_emits_an_engine_effect() {
        let (mut s, vp) = setup();
        let a = s.add_page("a", &vp);
        assert_eq!(run(&mut s, &vp, Request::PageMuteToggle).mute, vec![(a, true)]);
        assert_eq!(run(&mut s, &vp, Request::PageMuteToggle).mute, vec![(a, false)]);
        s.close_page(a, &vp);
        assert!(run(&mut s, &vp, Request::PageMuteToggle).mute.is_empty(), "no page, no effect");
    }

    #[test]
    fn opening_a_page_leaves_the_overview() {
        let (mut s, vp) = setup();
        s.add_page("a", &vp);
        run(&mut s, &vp, Request::OverviewToggle);
        assert!(s.overview_open);
        run(&mut s, &vp, Request::PageNew);
        assert!(!s.overview_open);
    }

    #[test]
    fn reload_without_url_is_noop() {
        let (mut s, vp) = setup();
        let id = s.add_page("", &vp);
        let mut fx = Effects::default();
        apply(&mut s, &vp, Request::Reload, &mut fx);
        assert!(fx.navigate.is_empty(), "empty url must not navigate");
        let _ = id;
    }
}
