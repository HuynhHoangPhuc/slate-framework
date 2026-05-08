//! Compile-fail test: TextSystem must NOT be Send.
//! This file should fail to compile with an error about Send trait.

fn assert_send<T: Send>() {}

fn main() {
    assert_send::<slate_framework::TextSystem>();
}
