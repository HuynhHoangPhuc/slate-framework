use slate_reactive::{Runtime, Signal, with_observer};

#[test]
fn get_set_untracked() {
    let rt = Runtime::new();
    let signal = Signal::new(rt, 42u32);

    assert_eq!(signal.get_untracked(), 42);
    signal.set(100);
    assert_eq!(signal.get_untracked(), 100);
}

#[test]
fn update_modifies_value() {
    let rt = Runtime::new();
    let signal = Signal::new(rt, String::from("hello"));

    signal.update(|s| s.push_str(" world"));
    assert_eq!(signal.get_untracked(), "hello world");
}

#[test]
fn subscription_via_get_in_observer_scope() {
    let rt = Runtime::new();
    let observer = rt.next_observer_id();
    let signal = Signal::new(rt.clone(), 0i32);

    with_observer(observer, || {
        let _ = signal.get();
    });

    let _ = rt.drain_dirty();
    signal.set(1);
    assert!(rt.drain_dirty());
}

#[test]
fn get_untracked_does_not_subscribe() {
    let rt = Runtime::new();
    let observer = rt.next_observer_id();
    let signal = Signal::new(rt.clone(), 0i32);

    with_observer(observer, || {
        let _ = signal.get_untracked();
    });

    rt.drain_dirty();
    signal.set(1);
    assert!(rt.drain_dirty());
}

#[test]
fn set_marks_runtime_dirty() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0);

    assert!(!rt.drain_dirty());
    signal.set(1);
    assert!(rt.drain_dirty());
}

#[test]
fn update_marks_runtime_dirty() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0);

    assert!(!rt.drain_dirty());
    signal.update(|n| *n += 1);
    assert!(rt.drain_dirty());
}

#[test]
fn clone_shares_state() {
    let rt = Runtime::new();
    let s1 = Signal::new(rt, vec![1, 2, 3]);
    let s2 = s1.clone();

    s1.update(|v| v.push(4));
    assert_eq!(s2.get_untracked(), vec![1, 2, 3, 4]);

    s2.set(vec![10, 20]);
    assert_eq!(s1.get_untracked(), vec![10, 20]);
}

#[test]
fn multiple_observers_all_subscribed() {
    let rt = Runtime::new();
    let o1 = rt.next_observer_id();
    let o2 = rt.next_observer_id();
    let signal = Signal::new(rt.clone(), 0);

    with_observer(o1, || {
        let _ = signal.get();
    });

    with_observer(o2, || {
        let _ = signal.get();
    });

    signal.set(1);
    assert!(rt.drain_dirty());
}

#[test]
fn try_get_semantics() {
    let rt = Runtime::new();
    let observer = rt.next_observer_id();
    let signal = Signal::new(rt.clone(), 42);

    assert_eq!(signal.try_get(), None);

    with_observer(observer, || {
        assert_eq!(signal.try_get(), Some(42));
    });

    assert_eq!(signal.try_get(), None);
}
