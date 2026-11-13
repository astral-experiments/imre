use super::*;
use crate::dynamic::ffi;

fn context() -> Context {
    ffi::new_context(
        serde_json::json!({"paths": []}),
        std::iter::empty::<String>(),
    )
}

#[test]
fn owned_registered_entry_roots_bound_slots_and_restores_its_frame() {
    unsafe extern "C" fn entry(raw: *mut Context, scope: Id) -> Id {
        // SAFETY: the worker owns its Context until the registered frame returns.
        let ctx = unsafe { &mut *raw };
        ctx.collect_unreachable();
        let result = (|| {
            let value = ctx.lookup(scope, "value")?;
            if ctx.lookup(scope, "alias")? != value {
                return Err(Error::new("AssertionError", "slot identity changed"));
            }
            if ctx.truth(value)? {
                return Err(Error::new("ValueError", "worker failed"));
            }
            Ok(value)
        })();
        match result {
            Ok(value) => value,
            Err(error) => {
                ctx.pending = Some(error);
                NONE
            }
        }
    }

    let mut ctx = context();
    let scope = ctx.scope("worker_test", None);
    let name = ctx.text("target");
    let signature = ctx.text(r#"{"positional":["value"],"kwonly":["alias"]}"#);
    let file = ctx.text("worker_test.py");
    let code = ctx.prelinked_code(
        crate::dynamic::value::Body::Native(entry, NONE),
        scope,
        name,
        signature,
        (name, NONE),
        (file, 1),
    );
    let defaults = ctx.dict(Vec::new());
    let target = ctx
        .define_function(code, scope, name, defaults, name, NONE, NONE, false)
        .unwrap();
    let operator = ctx.builtin("operator.call");
    let worker = std::cell::RefCell::new(ctx);
    for (forwarded, fails) in [(false, false), (false, true), (true, false), (true, true)] {
        let (value, frame) = {
            let mut ctx = worker.borrow_mut();
            let mark = ctx.heap.temporary_mark();
            let value = ctx.alloc(Value::List(if fails { vec![NONE] } else { Vec::new() }));
            // Collection in the linked entry must find the value through its bound scope.
            ctx.heap.release_temporaries_since(mark);
            let frame = if forwarded {
                let method = ctx.bind_python_method(target, value);
                let call = ctx
                    .prepare_call_in_scope(
                        scope,
                        operator,
                        vec![method],
                        vec![("alias".into(), value)],
                    )
                    .unwrap();
                let crate::dynamic::calls::RegisteredCall::Native(frame) = call else {
                    panic!("operator.call ran the registered method during preparation");
                };
                frame
            } else {
                ctx.prepare_bound_function(target, vec![value, value])
                    .unwrap()
            };
            // A parked native entry leaves the complete binding layout with its code.
            let layout = ctx.code_object(code).parsed_parameters.get().unwrap();
            assert_eq!(std::rc::Rc::strong_count(layout), 1);
            (value, frame)
        };
        // SAFETY: preparation released its RefMut. This thread owns the stable Context.
        let result = unsafe { frame.run(worker.as_ptr()) };
        let ctx = worker.borrow();
        assert_eq!(ctx.heap.python_depth(), 0);
        assert_eq!(ctx.call_depth, 0);
        assert!(ctx.pending.is_none());
        if fails {
            let error = result.unwrap_err();
            assert_eq!(error.kind, "ValueError");
            assert_eq!(error.message, "worker failed");
        } else {
            assert_eq!(result.unwrap(), value);
        }
    }
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

fn launch(
    ctx: &mut Context,
    operation: Operation,
    registration: Registration,
    roots: Vec<Id>,
) -> Result<(usize, Option<u64>)> {
    let request = super::Start::new(ctx, operation, registration, roots);
    let (ready, entry) = super::launch(request)?;
    ctx.workers.entries.push(entry);
    let started = ready.wait()?;
    let identity = (started.ident, started.native_id);
    started.publish()?;
    Ok(identity)
}

fn registered(handle: &Arc<Handle>, daemon: bool, owner: Id) -> Registration {
    Registration::Thread {
        handle: Arc::clone(handle),
        handle_id: NONE,
        thread: RegisteredThread { daemon, owner },
    }
}

#[test]
fn native_start_and_join_publish_the_worker_lifetime() {
    let mut ctx = context();
    let object = ctx.builtin("object");
    let owner = ctx.instance(object, None);
    let handle = Arc::<Handle>::default();
    while_lock_held(|lock| {
        let (ident, native_id) = launch(
            &mut ctx,
            Operation::Acquire {
                wait: Acquire::blocking(lock),
                release: None,
            },
            registered(&handle, false, owner),
            Vec::new(),
        )
        .unwrap();
        handle.join(Some(Duration::ZERO)).unwrap();
        assert!(handle.alive());
        assert!(ctx.workers.entries[0].handle.awaiting_exit());
        assert_eq!(ctx.interpreter.worker_count.load(Ordering::Acquire), 1);
        assert_eq!(ident, handle.ident());
        assert_ne!(ident, thread_ident());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_ne!(
            native_id.unwrap(),
            crate::objects::locks::native_thread_id()
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        assert_eq!(native_id, None);
    });
    handle.join(None).unwrap();
    handle.join(None).unwrap();
    assert!(!handle.alive());
    assert!(!ctx.workers.entries[0].handle.awaiting_exit());
    assert_eq!(ctx.interpreter.worker_count.load(Ordering::Acquire), 0);
}

#[test]
fn worker_waits_for_the_creator_to_publish_its_receiver() {
    let mut ctx = context();
    let scope = ctx.scope("worker_test", None);
    ctx.heap
        .enter(scope, NONE, crate::dynamic::value::FrameOwner::Module(NONE));
    let metaclass = ctx.builtin("type");
    let name = ctx.text("Worker");
    let bases = ctx.alloc(Value::Tuple(Vec::new()));
    let namespace = ctx.dict(Vec::new());
    let class = ctx
        .call(metaclass, vec![name, bases, namespace], vec![])
        .unwrap();
    for cancel in [true, false] {
        let owner = ctx.instance(class, None);
        let target = ctx.alloc(Value::List(Vec::new()));
        ctx.set_attr(owner, "_target", target).unwrap();
        let handle = Arc::<Handle>::default();
        let prefix = Arc::<PrimitiveLock>::default();
        let body = Arc::<PrimitiveLock>::default();
        assert!(prefix.acquire(false, None));
        let request = super::Start::new(
            &ctx,
            Operation::Acquire {
                wait: Acquire::blocking(&body),
                release: Some(prefix.clone()),
            },
            registered(&handle, false, owner),
            vec![target],
        );
        let (ready, entry) = super::launch(request).unwrap();
        ctx.workers.entries.push(entry);
        let started = ready.wait().unwrap();
        assert_ne!(started.ident, thread_ident());
        assert!(ctx.workers.roots().any(|id| *id == owner));
        assert!(prefix.locked());
        assert!(!body.locked());
        let ident = started.ident;
        if cancel {
            drop(started);
        } else {
            started.publish().unwrap();
        }
        assert!(handle.wait(Some(Duration::from_secs(5)), State::worker_pending));
        handle.join(Some(Duration::ZERO)).unwrap();
        ctx.reap_thread_operands();
        assert!(ctx.workers.entries.is_empty());
        assert_eq!(prefix.locked(), cancel);
        assert_eq!(body.locked(), !cancel);
        if cancel {
            assert_eq!(handle.ident(), 0);
            assert_eq!(ctx.lookup_attr(owner, "_target").unwrap(), target);
        } else {
            assert_eq!(handle.ident(), ident);
            assert_eq!(
                ctx.lookup_attr(owner, "_target").unwrap_err().kind,
                "AttributeError"
            );
        }
    }
    ctx.heap.leave(NONE, false);
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
    let handle = Arc::<Handle>::default();
    let weak_owner = ctx.heap.native_weak_reference(owner).unwrap();
    let (ident, _) = launch(
        &mut ctx,
        operation,
        registered(&handle, false, owner),
        operands,
    )
    .unwrap();
    let Value::List(items) = ctx.heap.get_mut(args) else {
        unreachable!()
    };
    items.clear();
    ctx.heap.weak_edge_removed(input);
    ctx.heap.release_temporaries_since(mark);
    ctx.heap.keep(weak_owner);
    ctx.collect_unreachable();
    assert!(matches!(ctx.heap.get(weak_owner), Value::Weak(wr) if wr.target == Some(owner)));
    assert!(matches!(ctx.heap.get(input), Value::Bytes(data) if data == b"abc"));
    assert!(handle.alive());
    assert_ne!(ident, thread_ident());
    assert!(ctx.check_thread_reporter_mutable().is_ok());

    drop(held);
    handle.join(None).unwrap();
    ctx.reap_thread_operands();
    assert!(ctx.workers.entries.is_empty());
    ctx.collect_unreachable();
    assert!(matches!(ctx.heap.get(weak_owner), Value::Weak(wr) if wr.target.is_none()));
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
    assert!(!ctx.workers.has_unreaped_workers());
    let mark = ctx.heap.temporary_mark();
    let object = ctx.builtin("object");
    let operand = ctx.instance(object, None);
    let weak = ctx.heap.native_weak_reference(operand).unwrap();

    let active = while_lock_held(|gate| {
        let start = prepare_detached(
            &mut ctx,
            Operation::Acquire {
                wait: Acquire::blocking(gate),
                release: None,
            },
            NONE,
            vec![operand],
        )
        .unwrap();
        // SAFETY: the prepared request belongs to this exclusively owned context.
        let ident = unsafe { start.run(&raw mut ctx) }.unwrap();
        let main = ctx.int(thread_ident() as i64);
        assert!(!ctx.equal(ident, main));
        assert!(ctx.workers.has_unreaped_workers());
        assert!(ctx.workers.shutdown_handles().next().is_none());
        assert!(ctx.workers.live_threads().next().is_none());
        assert!(ctx.check_thread_reporter_mutable().is_err());

        let owner = ctx.instance(object, None);
        let joined = Arc::<Handle>::default();
        launch(
            &mut ctx,
            Operation::Empty,
            registered(&joined, false, owner),
            Vec::new(),
        )
        .unwrap();
        joined.join(Some(Duration::from_secs(5))).unwrap();
        assert!(!joined.alive());
        ctx.reap_thread_operands();
        assert!(!ctx.workers.roots().any(|&id| id == owner));
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(weak);
        ctx.collect_unreachable();
        assert!(matches!(ctx.heap.get(weak), Value::Weak(wr) if wr.target == Some(operand)));
        Arc::clone(&ctx.workers.entries[0].handle)
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while active.awaiting_exit() {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(ctx.check_thread_reporter_mutable().is_ok());
    // A low-level worker can exit between reaping and enumerating GC roots.
    ctx.reap_thread_operands();
    assert!(ctx.workers.has_unreaped_workers());
    assert!(ctx.workers.roots().any(|&id| id == operand));
    assert!(!ctx.heap_and_roots().1.any(|id| id == operand));
    assert!(!ctx.workers.has_unreaped_workers());
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
        .spawn(move |_| {
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
    let object = ctx.builtin("object");
    let daemon_owner = ctx.instance(object, None);
    let normal_owner = ctx.instance(object, None);
    let daemon = Arc::<Handle>::default();
    let normal = Arc::<Handle>::default();
    while_lock_held(|lock| {
        launch(
            &mut ctx,
            Operation::Acquire {
                wait: Acquire::blocking(lock),
                release: None,
            },
            registered(&daemon, true, daemon_owner),
            Vec::new(),
        )
        .unwrap();
        launch(
            &mut ctx,
            Operation::Empty,
            registered(&normal, false, normal_owner),
            Vec::new(),
        )
        .unwrap();
        drop(ctx);
        assert!(normal.native.lock().unwrap().thread.is_none());
        assert!(daemon.alive());
        normal.join(None).unwrap();
    });
    daemon.join(None).unwrap();
}

#[test]
fn detached_handle_keeps_the_non_daemon_worker_registered_until_exit() {
    let mut ctx = context();
    let object = ctx.builtin("object");
    let owner = ctx.instance(object, None);
    let handle = Arc::<Handle>::default();
    assert_eq!(handle.ident(), 0);
    assert!(!handle.is_done());
    assert!(handle.set_done().is_err());
    let active = while_lock_held(|gate| {
        let (ident, _) = launch(
            &mut ctx,
            Operation::Acquire {
                wait: Acquire::blocking(gate),
                release: None,
            },
            registered(&handle, false, owner),
            Vec::new(),
        )
        .unwrap();
        assert_ne!(handle.ident(), thread_ident());
        assert_eq!(handle.ident(), ident);
        assert!(handle.check_new().is_err());
        handle.set_done().unwrap();
        let receiver = ctx.alloc(Value::ThreadHandle(handle.clone()));
        let join = ctx.lookup_attr(receiver, "join").unwrap();
        assert_eq!(ctx.call(join, vec![], vec![]).unwrap(), NONE);
        assert!(handle.is_done());
        assert!(handle.awaiting_exit());
        ctx.reap_thread_operands();
        assert_eq!(ctx.workers.entries.len(), 1);
        Arc::clone(&ctx.workers.entries[0].handle)
    });
    ctx.workers.join_non_daemon();
    assert!(!active.awaiting_exit());
    assert!(!handle.awaiting_exit());
    ctx.reap_thread_operands();
    assert!(ctx.workers.entries.is_empty());
}

#[test]
fn set_done_after_body_exit_releases_the_unclaimed_native_handle() {
    let handle = Arc::<Handle>::default();
    handle.spawn(|_| Ok(())).unwrap();
    assert!(handle.wait(Some(Duration::from_secs(5)), State::join_pending));
    // The body signalled exit, but no Python observer has joined its OS thread.
    assert!(handle.native.lock().unwrap().thread.is_some());
    handle.set_done().unwrap();
    assert!(handle.native.lock().unwrap().thread.is_none());
    assert!(handle.is_done());
    handle.join(None).unwrap();
}
