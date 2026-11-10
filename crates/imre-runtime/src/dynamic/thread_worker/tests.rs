use super::*;
use crate::dynamic::ffi;

fn context() -> Context {
    ffi::new_context(
        serde_json::json!({"paths": []}),
        std::iter::empty::<String>(),
    )
}

// An accidental blocking join must fail the live-state checks without hanging the test process.
fn while_lock_held<T>(probe: impl FnOnce(&Arc<PrimitiveLock>) -> T) -> T {
    let lock = Arc::new(PrimitiveLock::default());
    assert!(lock.acquire(false, None));
    thread::scope(|scope| {
        let (done, wait) = mpsc::channel::<()>();
        let held = Arc::clone(&lock);
        scope.spawn(move || {
            let _ = wait.recv_timeout(Duration::from_secs(5));
            assert!(held.release().is_ok());
        });
        let result = probe(&lock);
        drop(done);
        result
    })
}

#[test]
fn native_start_and_join_publish_the_worker_lifetime() {
    let ctx = context();
    let (worker, poll, alive, active, count) = while_lock_held(|lock| {
        let worker = launch(
            &ctx,
            Operation::Acquire {
                lock: Arc::clone(lock),
                release: None,
            },
            None,
            Arc::default(),
        )
        .unwrap();
        let poll = worker.handle.join(Some(Duration::ZERO));
        let alive = worker.handle.alive();
        let active = worker.active.load(Ordering::Acquire);
        let count = ctx.threads.count.load(Ordering::Acquire);
        (worker, poll, alive, active, count)
    });
    worker.handle.join(None).unwrap();
    worker.handle.join(None).unwrap();

    poll.unwrap();
    assert!(alive);
    assert!(active);
    assert_eq!(count, 1);
    assert_ne!(worker.ident, thread_ident());
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert_ne!(
        worker.native_id.unwrap(),
        crate::objects::locks::native_thread_id()
    );
    assert!(!worker.handle.alive());
    assert!(!worker.active.load(Ordering::Acquire));
    assert_eq!(ctx.threads.count.load(Ordering::Acquire), 0);
}

#[test]
fn bound_hash_worker_owns_expanded_input_while_running() {
    let mut ctx = context();
    let provider = crate::objects::HashState::provider("sha1")
        .unwrap_or_else(|_| panic!("expected the built-in sha1 provider"));
    let hash = SharedHash::new(provider.create(None, &[]));
    let digest = ctx.alloc(Value::Hash(hash.clone()));
    let mark = ctx.heap.temporary_mark();
    let input = ctx.alloc(Value::Bytes(b"abc".to_vec()));
    let args = ctx.alloc(Value::List(vec![input]));
    let kwargs = ctx.dict(Vec::new());
    let target = ctx.lookup_attr(digest, "update").unwrap();
    let object = ctx.builtin("object");
    let owner = ctx.instance(object, None);
    let (operation, operands) = worker_target(&ctx, target, args, kwargs).unwrap();

    // Hold the worker inside the native update while the caller clears the original list.
    let held = hash.lock();
    let worker = launch(&ctx, operation, None, Arc::default()).unwrap();
    ctx.workers.entries.push(Entry {
        thread: Some(RegisteredThread {
            handle: Arc::clone(&worker.handle),
            daemon: false,
            owner: Some(owner),
        }),
        active: worker.active,
        operands,
    });
    let Value::List(items) = ctx.heap.get_mut(args) else {
        unreachable!()
    };
    items.clear();
    ctx.heap.weak_edge_removed(input);
    ctx.heap.release_temporaries_since(mark);
    ctx.collect_unreachable();
    assert!(matches!(ctx.heap.get(input), Value::Bytes(data) if data == b"abc"));
    assert!(worker.handle.alive());
    assert_ne!(worker.ident, thread_ident());
    assert!(ctx.check_thread_reporter_mutable().is_ok());

    drop(held);
    worker.handle.join(None).unwrap();
    ctx.reap_thread_operands();
    assert!(ctx.workers.entries.is_empty());
    let hexdigest = ctx.lookup_attr(digest, "hexdigest").unwrap();
    let result = ctx.call(hexdigest, Vec::new(), Vec::new()).unwrap();
    assert_eq!(
        ctx.str_value(result).unwrap(),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

#[test]
fn joined_and_detached_operands_are_released_at_their_own_checkpoints() {
    let mut ctx = context();
    let mark = ctx.heap.temporary_mark();
    let object = ctx.builtin("object");
    let operand = ctx.instance(object, None);
    let weak = ctx.heap.native_weak_reference(operand).unwrap();

    let active = while_lock_held(|gate| {
        let ident = start_detached(
            &mut ctx,
            Operation::Acquire {
                lock: Arc::clone(gate),
                release: None,
            },
            NONE,
            vec![operand],
        )
        .unwrap();
        assert_ne!(ident, thread_ident());
        assert!(ctx.workers.shutdown_handles().next().is_none());
        assert!(ctx.workers.live_threads().next().is_none());
        assert!(ctx.check_thread_reporter_mutable().is_err());

        let owner = ctx.instance(object, None);
        let joined = launch(&ctx, Operation::Empty, None, Arc::default()).unwrap();
        joined.handle.join(Some(Duration::from_secs(5))).unwrap();
        assert!(!joined.handle.alive());
        ctx.workers.entries.push(Entry {
            thread: Some(RegisteredThread {
                handle: joined.handle,
                daemon: false,
                owner: Some(owner),
            }),
            active: joined.active,
            operands: vec![],
        });
        ctx.reap_thread_operands();
        assert!(!ctx.workers.roots().any(|&id| id == owner));
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(weak);
        ctx.collect_unreachable();
        assert!(matches!(ctx.heap.get(weak), Value::Weak(wr) if wr.target == Some(operand)));
        Arc::clone(&ctx.workers.entries[0].active)
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while active.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(ctx.check_thread_reporter_mutable().is_ok());
    // A low-level worker can exit between reaping and enumerating GC roots.
    ctx.reap_thread_operands();
    assert!(ctx.workers.roots().any(|&id| id == operand));
    assert!(!ctx.heap_and_roots().1.any(|id| id == operand));
    ctx.collect_unreachable();
    assert!(matches!(ctx.heap.get(weak), Value::Weak(wr) if wr.target.is_none()));
}

#[test]
fn zero_timeout_applies_to_worker_exit_and_all_observers_share_native_join() {
    let timeout = Duration::from_secs(5);
    // Bound every wait so an incorrect join cannot strand the test process.
    struct ExitGate {
        entered: mpsc::Sender<()>,
        poll: mpsc::Receiver<()>,
        returned: mpsc::Receiver<()>,
        completed: mpsc::Sender<()>,
    }
    impl Drop for ExitGate {
        fn drop(&mut self) {
            let _ = self.entered.send(());
            let _ = self.poll.recv_timeout(Duration::from_secs(5));
            let _ = self.returned.recv_timeout(Duration::from_millis(250));
            let _ = self.completed.send(());
        }
    }
    thread_local! {
        static GATE: std::cell::OnceCell<ExitGate> = const { std::cell::OnceCell::new() };
    }

    let (end_body, body) = mpsc::channel();
    let (entered, at_exit) = mpsc::channel();
    let (polling, poll) = mpsc::channel();
    let (finished, returned) = mpsc::channel();
    let (completed, completion) = mpsc::channel();
    let handle = Arc::<Handle>::default();
    handle
        .spawn(move || {
            let gate = ExitGate {
                entered,
                poll,
                returned,
                completed,
            };
            GATE.with(|cell| assert!(cell.set(gate).is_ok()));
            body.recv_timeout(timeout).unwrap();
            Err("worker failure".into())
        })
        .unwrap();

    handle.join(Some(Duration::ZERO)).unwrap();
    assert!(handle.alive());
    end_body.send(()).unwrap();
    at_exit.recv_timeout(timeout).unwrap();
    thread::scope(|scope| {
        let observer = Arc::clone(&handle);
        scope.spawn(move || assert!(!observer.alive()));
        let deadline = std::time::Instant::now() + timeout;
        while handle.native.try_lock().is_ok() {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        // is_alive() owns the OS join while TLS is shutting down. Once exit was
        // signaled, even join(0) must share that join and receive its saved error.
        polling.send(()).unwrap();
        let error = handle.join(Some(Duration::ZERO)).unwrap_err();
        completion
            .try_recv()
            .expect("native TLS teardown completed before join returned");
        let _ = finished.send(());
        assert_eq!(error.kind, "SystemError");
        assert_eq!(error.message, "worker failure");
    });
    handle.join(None).unwrap();
}

#[test]
fn shutdown_joins_non_daemon_workers_and_leaves_daemon_workers_running() {
    let mut ctx = context();
    let (daemon, daemon_alive, non_daemon_joined) = while_lock_held(|lock| {
        let daemon = launch(
            &ctx,
            Operation::Acquire {
                lock: Arc::clone(lock),
                release: None,
            },
            None,
            Arc::default(),
        )
        .unwrap();
        let normal = launch(&ctx, Operation::Empty, None, Arc::default()).unwrap();
        for (worker, is_daemon) in [(&daemon, true), (&normal, false)] {
            ctx.workers.entries.push(Entry {
                thread: Some(RegisteredThread {
                    handle: Arc::clone(&worker.handle),
                    daemon: is_daemon,
                    owner: None,
                }),
                active: Arc::clone(&worker.active),
                operands: Vec::new(),
            });
        }
        drop(ctx);
        let non_daemon_joined = normal.handle.native.lock().unwrap().thread.is_none();
        let daemon_alive = daemon.handle.alive();
        normal.handle.join(None).unwrap();
        (daemon, daemon_alive, non_daemon_joined)
    });
    daemon.handle.join(None).unwrap();
    assert!(daemon_alive);
    assert!(non_daemon_joined);
}

#[test]
fn detached_handle_keeps_the_non_daemon_worker_registered_until_exit() {
    let mut ctx = context();
    let handle = Arc::<Handle>::default();
    assert_eq!(handle.ident(), 0);
    assert!(!handle.is_done());
    assert!(handle.set_done().is_err());
    let active = while_lock_held(|gate| {
        let worker = launch(
            &ctx,
            Operation::Acquire {
                lock: Arc::clone(gate),
                release: None,
            },
            None,
            Arc::clone(&handle),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&worker.handle, &handle));
        assert_ne!(handle.ident(), thread_ident());
        assert_eq!(handle.ident(), worker.ident);
        assert!(handle.check_new().is_err());
        ctx.workers.entries.push(Entry {
            thread: Some(RegisteredThread {
                handle: worker.handle,
                daemon: false,
                owner: None,
            }),
            operands: Vec::new(),
            active: Arc::clone(&worker.active),
        });
        handle.set_done().unwrap();
        handle.join_public(&mut ctx, None).unwrap();
        assert!(handle.is_done());
        assert!(handle.awaiting_exit());
        ctx.reap_thread_operands();
        assert_eq!(ctx.workers.entries.len(), 1);
        worker.active
    });
    ctx.workers.join_non_daemon();
    assert!(!active.load(Ordering::Acquire));
    assert!(!handle.awaiting_exit());
    ctx.reap_thread_operands();
    assert!(ctx.workers.entries.is_empty());
}

#[test]
fn set_done_after_body_exit_releases_the_unclaimed_native_handle() {
    let handle = Arc::<Handle>::default();
    handle.spawn(|| Ok(())).unwrap();
    assert!(handle.wait(Some(Duration::from_secs(5)), State::join_pending));
    // The body signalled exit, but no Python observer has joined its OS thread.
    assert!(handle.native.lock().unwrap().thread.is_some());
    handle.set_done().unwrap();
    assert!(handle.native.lock().unwrap().thread.is_none());
    assert!(handle.is_done());
    handle.join(None).unwrap();
}
