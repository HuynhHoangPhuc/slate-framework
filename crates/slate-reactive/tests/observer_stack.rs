use slate_reactive::{ObserverId, Runtime, current_observer, with_observer};

#[test]
fn nested_observers_pop_order() {
    let rt = Runtime::new();
    let a = rt.next_observer_id();
    let b = rt.next_observer_id();
    let c = rt.next_observer_id();

    assert_eq!(current_observer(), None);

    with_observer(a, || {
        assert_eq!(current_observer(), Some(a));

        with_observer(b, || {
            assert_eq!(current_observer(), Some(b));

            with_observer(c, || {
                assert_eq!(current_observer(), Some(c));
            });

            assert_eq!(current_observer(), Some(b));
        });

        assert_eq!(current_observer(), Some(a));
    });

    assert_eq!(current_observer(), None);
}

#[test]
fn panic_inside_observer_pops_correctly() {
    let rt = Runtime::new();
    let a = rt.next_observer_id();
    let b = rt.next_observer_id();

    with_observer(a, || {
        let result = std::panic::catch_unwind(|| {
            with_observer(b, || {
                assert_eq!(current_observer(), Some(b));
                panic!("intentional panic inside inner observer");
            });
        });

        assert!(result.is_err(), "inner scope should have panicked");
        assert_eq!(
            current_observer(),
            Some(a),
            "outer observer should still be active after inner panic"
        );
    });

    assert_eq!(
        current_observer(),
        None,
        "stack should be empty after all scopes exit"
    );
}

#[test]
fn deeply_nested_observers() {
    let rt = Runtime::new();
    let ids: Vec<ObserverId> = (0..10).map(|_| rt.next_observer_id()).collect();

    fn recurse(ids: &[ObserverId], depth: usize) {
        if depth >= ids.len() {
            return;
        }

        with_observer(ids[depth], || {
            assert_eq!(current_observer(), Some(ids[depth]));
            recurse(ids, depth + 1);
            assert_eq!(current_observer(), Some(ids[depth]));
        });
    }

    recurse(&ids, 0);
    assert_eq!(current_observer(), None);
}

#[test]
fn observer_scope_isolated_per_thread() {
    let rt = Runtime::new();
    let main_observer = rt.next_observer_id();

    with_observer(main_observer, || {
        assert_eq!(current_observer(), Some(main_observer));

        let rt_clone = rt.clone();
        let handle = std::thread::spawn(move || {
            assert_eq!(
                current_observer(),
                None,
                "new thread should have empty observer stack"
            );

            let thread_observer = rt_clone.next_observer_id();
            with_observer(thread_observer, || {
                assert_eq!(current_observer(), Some(thread_observer));
            });

            assert_eq!(current_observer(), None);
        });

        handle.join().expect("thread should not panic");

        assert_eq!(
            current_observer(),
            Some(main_observer),
            "main thread observer should be unaffected"
        );
    });
}
