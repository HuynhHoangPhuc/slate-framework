//! IME event dispatch + `WindowImeDelegate` impl.
//!
//! All four event dispatchers share the focused-chain bubble shape used by
//! keyboard dispatch — snapshot per-element handlers up-front, then walk
//! App-level handlers if propagation was not stopped.
//!
//! `republish_ime_cache` recomputes the snapshot consumed by `WindowImeDelegate`
//! at the end of `dispatch_ime_preedit` and at the end of every paint pass.
//! `drain_pending_ime_ops` runs queued [`PendingImeOp`]s before
//! `apply_pending_focus_op` so Tab-during-composition lands its synthetic
//! commit on the still-focused element.

use std::time::Instant;

use slate_platform::{PhysicalRect, WindowId, WindowImeDelegate};
use smallvec::SmallVec;

use crate::event::{
    ElementImeCommitHandler, ElementImeLifecycleHandler, ElementImePreeditHandler, EventCtx,
    ImeCommitEvent, ImeCommitHandler, ImeLifecycleEvent, ImeLifecycleHandler, ImePreeditEvent,
    ImePreeditHandler, PendingCaptureOp, PendingFocusOp,
};
use crate::ime::{CachedImeQuery, PendingImeOp};
use crate::types::ElementId;

use super::super::state::AppState;
use super::super::types::AppSignal;

impl AppState {
    /// Install App-level IME handlers. Called once by `App::run` after
    /// construction, mirrors `install_key_handlers`.
    pub(crate) fn install_ime_handlers(
        &self,
        on_ime_preedit: Vec<ImePreeditHandler>,
        on_ime_commit: Vec<ImeCommitHandler>,
        on_ime_enabled: Vec<ImeLifecycleHandler>,
        on_ime_disabled: Vec<ImeLifecycleHandler>,
    ) {
        *self.on_ime_preedit.borrow_mut() = on_ime_preedit;
        *self.on_ime_commit.borrow_mut() = on_ime_commit;
        *self.on_ime_enabled.borrow_mut() = on_ime_enabled;
        *self.on_ime_disabled.borrow_mut() = on_ime_disabled;
    }

    /// Recompute the snapshot consumed by [`WindowImeDelegate`] sync queries
    /// from the currently-focused element's [`ImeState`].
    ///
    /// Called at the end of `dispatch_ime_preedit` AND at the end of every
    /// paint pass — those are the two deterministic moments the OS sync-query
    /// channel must see updated values.
    pub(crate) fn republish_ime_cache(&self, window_id: WindowId) {
        let mut snap = CachedImeQuery::default();

        let focused = {
            let guard = self.windows.borrow();
            guard.get(&window_id).and_then(|w| w.focus_registry.borrow().focused())
        };
        let Some(focused) = focused else {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.cached_ime_query.borrow_mut() = snap;
            }
            return;
        };

        let entry = {
            let guard = self.windows.borrow();
            guard.get(&window_id).and_then(|w| w.ime_registry.borrow().get(focused))
        };
        if let Some(state_rc) = entry
            && let Ok(state) = state_rc.try_borrow()
        {
            snap.caret_client_rect = state.caret_client_rect;
            snap.marked_range = state.answer_marked_range();
            snap.selected_range = state.answer_selected_range();

            const HALF: usize = 256;
            let len = state.text.len();
            let mut lo = state.caret.saturating_sub(HALF);
            let mut hi = (state.caret + HALF).min(len);
            while lo > 0 && !state.text.is_char_boundary(lo) {
                lo -= 1;
            }
            while hi < len && !state.text.is_char_boundary(hi) {
                hi += 1;
            }
            if let Some(slice) = state.text.get(lo..hi) {
                snap.text_window = Some((lo..hi, slice.to_string()));
            }
        }

        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window_id) {
            *win.cached_ime_query.borrow_mut() = snap;
        }
    }

    /// Drain queued [`PendingImeOp`]s for the given window. Called BEFORE
    /// `apply_pending_focus_op` so Tab-during-composition lands its synthetic
    /// commit on the still-focused element.
    pub(crate) fn drain_pending_ime_ops(&self, window_id: WindowId) {
        let ops: Vec<PendingImeOp> = {
            let guard = self.windows.borrow();
            match guard.get(&window_id) {
                Some(win) => win.pending_ime_ops.borrow_mut().drain(..).collect(),
                None => return,
            }
        };
        for op in ops {
            match op {
                PendingImeOp::Commit { text, .. } => {
                    let focused = {
                        let guard = self.windows.borrow();
                        guard.get(&window_id).and_then(|w| w.focus_registry.borrow().focused())
                    };
                    let Some(focused) = focused else { continue };
                    let state_rc = {
                        let guard = self.windows.borrow();
                        guard.get(&window_id).and_then(|w| w.ime_registry.borrow().get(focused))
                    };
                    let Some(state_rc) = state_rc else { continue };
                    if let Ok(mut state) = state_rc.try_borrow_mut() {
                        state.preedit = None;
                        if !text.is_empty() {
                            let caret = state.caret;
                            state.text.insert_str(caret, &text);
                            state.caret = caret + text.len();
                        }
                    }
                }
            }
        }
    }

    /// Dispatch `Event::ImeEnabled` — focused-chain bubble, no payload.
    pub(crate) fn dispatch_ime_enabled(&self, window: WindowId) -> AppSignal {
        let has_app_handlers = !self.on_ime_enabled.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = ImeLifecycleEvent { timestamp: Instant::now() };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        let focused = {
            let guard = self.windows.borrow();
            guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
        };

        let element_handlers: SmallVec<[ElementImeLifecycleHandler; 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::None };
            let map = win.ime_handler_map.borrow();
            chain.iter().filter_map(|id| map.get(id).and_then(|h| h.on_ime_enabled.clone())).collect()
        };

        for handler in &element_handlers {
            let id = chain.first().copied().unwrap_or_else(ElementId::root);
            let ime_rc = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { break };
                win.ime_registry.clone()
            };
            let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                )
                .with_ime(id, &ime_rc);
            handler(&event, &mut ctx);
            if stopped { break; }
        }

        if !stopped && has_app_handlers {
            let mut handlers = self.on_ime_enabled.borrow_mut();
            for handler in handlers.iter_mut() {
                let id = chain.first().copied().unwrap_or_else(ElementId::root);
                let ime_rc = {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    win.ime_registry.clone()
                };
                let mut ctx = EventCtx::new(
                        &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                    )
                    .with_ime(id, &ime_rc);
                handler(&event, &mut ctx);
                if stopped { break; }
            }
            drop(handlers);
        }

        self.drain_pending_ime_ops(window);
        self.apply_pending_focus_op(window, pending_focus_op);
        self.apply_pending_capture_op(window, pending_capture_op);
        AppSignal::RequestRedraw { window }
    }

    /// Dispatch `Event::ImeDisabled` — focused-chain bubble, no payload.
    pub(crate) fn dispatch_ime_disabled(&self, window: WindowId) -> AppSignal {
        let has_app_handlers = !self.on_ime_disabled.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = ImeLifecycleEvent { timestamp: Instant::now() };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        let focused = {
            let guard = self.windows.borrow();
            guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
        };

        let element_handlers: SmallVec<[ElementImeLifecycleHandler; 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::None };
            let map = win.ime_handler_map.borrow();
            chain.iter().filter_map(|id| map.get(id).and_then(|h| h.on_ime_disabled.clone())).collect()
        };

        for handler in &element_handlers {
            let id = chain.first().copied().unwrap_or_else(ElementId::root);
            let ime_rc = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { break };
                win.ime_registry.clone()
            };
            let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                )
                .with_ime(id, &ime_rc);
            handler(&event, &mut ctx);
            if stopped { break; }
        }

        if !stopped && has_app_handlers {
            let mut handlers = self.on_ime_disabled.borrow_mut();
            for handler in handlers.iter_mut() {
                let id = chain.first().copied().unwrap_or_else(ElementId::root);
                let ime_rc = {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    win.ime_registry.clone()
                };
                let mut ctx = EventCtx::new(
                        &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                    )
                    .with_ime(id, &ime_rc);
                handler(&event, &mut ctx);
                if stopped { break; }
            }
            drop(handlers);
        }

        self.drain_pending_ime_ops(window);
        self.apply_pending_focus_op(window, pending_focus_op);
        self.apply_pending_capture_op(window, pending_capture_op);
        AppSignal::RequestRedraw { window }
    }

    /// Dispatch `Event::ImePreedit` — focused-chain bubble. Republishes the
    /// IME cache at the end so OS sync queries that arrive between paints
    /// read fresh values.
    pub(crate) fn dispatch_ime_preedit(
        &self,
        window: WindowId,
        text: String,
        cursor_byte_offset: usize,
        selection: Option<std::ops::Range<usize>>,
    ) -> AppSignal {
        let has_app_handlers = !self.on_ime_preedit.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = ImePreeditEvent {
            text,
            cursor_byte_offset,
            selection,
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        let focused = {
            let guard = self.windows.borrow();
            guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
        };

        let element_handlers: SmallVec<[ElementImePreeditHandler; 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::None };
            let map = win.ime_handler_map.borrow();
            chain.iter().filter_map(|id| map.get(id).and_then(|h| h.on_ime_preedit.clone())).collect()
        };

        for handler in &element_handlers {
            let id = chain.first().copied().unwrap_or_else(ElementId::root);
            let ime_rc = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { break };
                win.ime_registry.clone()
            };
            let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                )
                .with_ime(id, &ime_rc);
            handler(&event, &mut ctx);
            if stopped { break; }
        }

        if !stopped && has_app_handlers {
            let mut handlers = self.on_ime_preedit.borrow_mut();
            for handler in handlers.iter_mut() {
                let id = chain.first().copied().unwrap_or_else(ElementId::root);
                let ime_rc = {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    win.ime_registry.clone()
                };
                let mut ctx = EventCtx::new(
                        &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                    )
                    .with_ime(id, &ime_rc);
                handler(&event, &mut ctx);
                if stopped { break; }
            }
            drop(handlers);
        }

        self.drain_pending_ime_ops(window);
        self.apply_pending_focus_op(window, pending_focus_op);
        self.apply_pending_capture_op(window, pending_capture_op);
        self.republish_ime_cache(window);
        AppSignal::RequestRedraw { window }
    }

    /// Dispatch `Event::ImeCommit` — focused-chain bubble.
    pub(crate) fn dispatch_ime_commit(&self, window: WindowId, text: String) -> AppSignal {
        let has_app_handlers = !self.on_ime_commit.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = ImeCommitEvent {
            text,
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        let focused = {
            let guard = self.windows.borrow();
            guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
        };

        let element_handlers: SmallVec<[ElementImeCommitHandler; 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::None };
            let map = win.ime_handler_map.borrow();
            chain.iter().filter_map(|id| map.get(id).and_then(|h| h.on_ime_commit.clone())).collect()
        };

        for handler in &element_handlers {
            let id = chain.first().copied().unwrap_or_else(ElementId::root);
            let ime_rc = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { break };
                win.ime_registry.clone()
            };
            let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                )
                .with_ime(id, &ime_rc);
            handler(&event, &mut ctx);
            if stopped { break; }
        }

        if !stopped && has_app_handlers {
            let mut handlers = self.on_ime_commit.borrow_mut();
            for handler in handlers.iter_mut() {
                let id = chain.first().copied().unwrap_or_else(ElementId::root);
                let ime_rc = {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    win.ime_registry.clone()
                };
                let mut ctx = EventCtx::new(
                        &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                    )
                    .with_ime(id, &ime_rc);
                handler(&event, &mut ctx);
                if stopped { break; }
            }
            drop(handlers);
        }

        self.drain_pending_ime_ops(window);
        self.apply_pending_focus_op(window, pending_focus_op);
        self.apply_pending_capture_op(window, pending_capture_op);
        AppSignal::RequestRedraw { window }
    }
}

// ---------------------------------------------------------------------------
// WindowImeDelegate impl — answers OS sync queries from the cached snapshot
// only. NEVER traverses the live `ime_registry` (avoids re-entrancy).
// ---------------------------------------------------------------------------

impl WindowImeDelegate for AppState {
    fn ime_caret_rect(&self, window_id: WindowId) -> Option<PhysicalRect> {
        let guard = self.windows.borrow();
        guard.get(&window_id).and_then(|w| w.cached_ime_query.borrow().caret_client_rect)
    }

    fn ime_text(&self, window_id: WindowId, range: std::ops::Range<usize>) -> Option<String> {
        let guard = self.windows.borrow();
        let win = guard.get(&window_id)?;
        let cache = win.cached_ime_query.borrow();
        let (window_range, text) = cache.text_window.as_ref()?;
        if range.start < window_range.start || range.end > window_range.end {
            return None;
        }
        let lo = range.start - window_range.start;
        let hi = range.end - window_range.start;
        text.get(lo..hi).map(|s| s.to_string())
    }

    fn ime_selected_range(&self, window_id: WindowId) -> Option<std::ops::Range<usize>> {
        let guard = self.windows.borrow();
        guard.get(&window_id).and_then(|w| w.cached_ime_query.borrow().selected_range.clone())
    }

    fn ime_marked_range(&self, window_id: WindowId) -> Option<std::ops::Range<usize>> {
        let guard = self.windows.borrow();
        guard.get(&window_id).and_then(|w| w.cached_ime_query.borrow().marked_range.clone())
    }
}
