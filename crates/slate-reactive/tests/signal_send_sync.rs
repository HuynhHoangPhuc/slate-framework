use slate_reactive::Signal;

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn signal_u32_is_send_sync() {
    assert_send::<Signal<u32>>();
    assert_sync::<Signal<u32>>();
    assert_send_sync::<Signal<u32>>();
}

#[test]
fn signal_string_is_send_sync() {
    assert_send::<Signal<String>>();
    assert_sync::<Signal<String>>();
    assert_send_sync::<Signal<String>>();
}

#[test]
fn signal_vec_is_send_sync() {
    assert_send::<Signal<Vec<u8>>>();
    assert_sync::<Signal<Vec<u8>>>();
    assert_send_sync::<Signal<Vec<u8>>>();
}

#[test]
fn signal_arc_is_send_sync() {
    use std::sync::Arc;
    assert_send::<Signal<Arc<String>>>();
    assert_sync::<Signal<Arc<String>>>();
    assert_send_sync::<Signal<Arc<String>>>();
}

#[test]
fn signal_can_be_sent_to_thread() {
    use slate_reactive::Runtime;
    use std::thread;

    let rt = Runtime::new();
    let signal = Signal::new(rt, 42u32);

    let handle = thread::spawn(move || {
        assert_eq!(signal.get_untracked(), 42);
        signal.set(100);
        signal.get_untracked()
    });

    let result = handle.join().unwrap();
    assert_eq!(result, 100);
}
