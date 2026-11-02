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
        )
        .unwrap();
        let normal = launch(&ctx, Operation::Empty, None).unwrap();
        for (worker, is_daemon) in [(&daemon, true), (&normal, false)] {
            ctx.workers.entries.push(Entry {
                handle: Rc::clone(&worker.handle),
                daemon: is_daemon,
                owner: None,
                ident: worker.ident,
                active: Arc::clone(&worker.active),
                operands: Vec::new(),
            });
        }
        drop(ctx);
        let non_daemon_joined = normal.handle.thread.borrow().is_none();
        let daemon_alive = daemon.handle.alive();
        normal.handle.join(None).unwrap();
        (daemon, daemon_alive, non_daemon_joined)
    });
    daemon.handle.join(None).unwrap();
    assert!(daemon_alive);
    assert!(non_daemon_joined);
}
