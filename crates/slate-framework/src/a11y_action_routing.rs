//! Shared screen-reader action routing.
//!
//! Maps an `accesskit::Action` (as delivered by either platform's AccessKit
//! adapter) onto the platform-neutral [`A11yAction`] Slate can synthesise.
//! Both the macOS (VoiceOver) and Windows (Narrator) adapters route through
//! this single function so the two never diverge on which AT actions Slate
//! honours.
//!
//! The mapping is pure (no I/O), so it is unit-tested directly without a live
//! assistive client.

use accesskit::{Action, ActionRequest};
use slate_platform::A11yAction;

/// Translate an AccessKit [`ActionRequest`] into the Slate action Slate can
/// synthesise, or `None` for actions Slate does not route.
///
/// `Action::Focus` → move keyboard focus to the node. `Action::Click` (the
/// screen-reader "press"/default action) → activate the node. Every other
/// AccessKit action is dropped here and never reaches the platform seam.
pub(crate) fn route_action_request(request: &ActionRequest) -> Option<(u64, A11yAction)> {
    let node = request.target_node.0;
    match request.action {
        Action::Focus => Some((node, A11yAction::Focus)),
        Action::Click => Some((node, A11yAction::Activate)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit::{NodeId, TreeId, Uuid};

    fn request(action: Action, node: u64) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: TreeId(Uuid::nil()),
            target_node: NodeId(node),
            data: None,
        }
    }

    #[test]
    fn focus_action_maps_to_focus() {
        assert_eq!(
            route_action_request(&request(Action::Focus, 42)),
            Some((42, A11yAction::Focus))
        );
    }

    #[test]
    fn click_action_maps_to_activate() {
        // A screen reader's default "press" arrives as Action::Click.
        assert_eq!(
            route_action_request(&request(Action::Click, 7)),
            Some((7, A11yAction::Activate))
        );
    }

    #[test]
    fn unrouted_actions_are_dropped() {
        for action in [Action::Blur, Action::Increment, Action::ScrollDown] {
            assert_eq!(route_action_request(&request(action, 1)), None);
        }
    }

    #[test]
    fn target_node_id_is_preserved() {
        let id = 0xDEAD_BEEF_u64;
        assert_eq!(
            route_action_request(&request(Action::Focus, id)),
            Some((id, A11yAction::Focus))
        );
    }
}
