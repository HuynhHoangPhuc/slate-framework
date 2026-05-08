//! Compile-fail test: external crates must NOT be able to impl IntoElement.
//! The sealed pattern prevents this — `IntoElement: sealed::Sealed` and
//! `Sealed` is `pub(crate)`, so this file (treated as an external user) cannot
//! satisfy the bound.

struct ExternalType;

impl slate_framework::IntoElement for ExternalType {
    type Element = ExternalType;
    fn into_element(self) -> Self::Element {
        self
    }
}

fn main() {}
