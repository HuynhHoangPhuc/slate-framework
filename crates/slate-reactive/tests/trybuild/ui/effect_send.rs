use slate_reactive::{Effect, ReactiveOwner, Runtime};

fn assert_send<T: Send>(_: T) {}

fn main() {
    let rt = Runtime::new();
    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let effect = Effect::new(rt, || {});

    assert_send(effect);
}
