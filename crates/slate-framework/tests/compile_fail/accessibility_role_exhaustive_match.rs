//! `AccessibilityRole` is `#[non_exhaustive]`, so an exhaustive `match` in a
//! downstream crate (this trybuild case is compiled as one) MUST include a
//! wildcard arm. Omitting it is a compile error — this guards the forward-
//! compatibility contract: adding a new role variant never breaks adopters.

use slate_framework::types::AccessibilityRole;

fn role_label(role: AccessibilityRole) -> &'static str {
    // No wildcard arm: rejected because the enum is non-exhaustive.
    match role {
        AccessibilityRole::Unknown => "unknown",
        AccessibilityRole::Button => "button",
    }
}

fn main() {
    let _ = role_label(AccessibilityRole::Button);
}
