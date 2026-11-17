use super::*;
use crate::dynamic::ffi;

fn status(ctx: &mut Context, handle: &Arc<Handle>, query: Status) -> bool {
    let receiver = ctx.alloc(Value::ThreadHandle(Arc::clone(handle)));
    query.call(ctx, &[receiver], &[]).unwrap() == boolean(true)
}

fn context() -> Context {
    ffi::new_context(
        serde_json::json!({"paths": []}),
        std::iter::empty::<String>(),
    )
}

#[test]
fn each_execution_traces_its_pending_error_until_it_is_consumed() {
    fn check(ctx: &mut Context, wait: impl FnOnce()) -> (bool, bool) {
        // External interpreter roots stay the same while this execution raises and clears.
        let globals: Vec<_> = ctx.heap_and_roots().1.collect();
        let mark = ctx.heap.temporary_mark();
        let value = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        let watch = ctx
            .heap
            .new_weak_reference(value, NONE, crate::dynamic::weakref::Kind::Reference, None)
            .unwrap();
        let mut error = Error::new("ValueError", "");
        error.args = Some(ctx.alloc(Value::Tuple(vec![value])));
        ctx.heap.execution.pending = Some(error);
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(watch);
        wait();
        ctx.heap.collect(globals.iter().copied());
        let held = ctx.heap.get(watch).weak_reference().unwrap().target == Some(value);
        ctx.heap.execution.pending = None;
        ctx.heap.collect(globals.into_iter());
        let cleared = ctx
            .heap
            .get(watch)
            .weak_reference()
            .unwrap()
            .target
            .is_none();
        (held, cleared)
    }

    let mut creator = context();
    let interpreter = creator.interpreter.clone();
    let (ready, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let timeout = Duration::from_secs(3);
    thread::scope(|scope| {
        let child = scope.spawn(move || {
            let mut child = ffi::new_worker_context(interpreter, Heap::new());
            check(&mut child, || {
                ready.send(thread_ident()).unwrap();
                released.recv_timeout(timeout).unwrap();
            })
        });
        let creator = check(&mut creator, || {
            assert_ne!(started.recv_timeout(timeout).unwrap(), thread_ident())
        });
        release.send(()).unwrap();
        assert_eq!(
            (creator, child.join().unwrap()),
            ((true, true), (true, true))
        );
    });
}

#[test]
fn detached_start_dispatches_its_audit_hooks_when_invoked() {
    let mut ctx = context();
    let start = ctx.builtin("_thread.start_new_thread");
    let target = ctx.builtin("_thread.exit");
    let positional = ctx.alloc(Value::Tuple(Vec::new()));
    let hook = ctx.builtin("len");
    ctx.audit_hooks.push(hook);

    let call = ctx
        .prepare_call(start, vec![target, positional].into(), vec![])
        .expect("preparing the start must not call the audit hook");
    // SAFETY: this test owns the context and its original argument roots.
    let error = unsafe { call.run(&raw mut ctx) }.unwrap_err();
    assert_eq!(error.kind, "TypeError");
    assert!(error.message.contains("len"));
    assert!(!ctx.heap.workers.has_unreaped_workers());
}

#[test]
fn bound_release_signals_the_selected_native_waiter() {
    let mut ctx = context();
    let gate = Arc::<PrimitiveLock>::default();
    assert!(gate.acquire(false, None));
    let waiter = ctx.alloc(Value::PrimitiveLock(Arc::clone(&gate)));
    let release = ctx.lookup_attr(waiter, "release").unwrap();
    let call = ctx.prepare_call(release, vec![].into(), vec![]).unwrap();
    let crate::dynamic::calls::RegisteredCall::Release(selected) = call else {
        panic!("bound lock release was not prepared");
    };
    assert!(gate.locked());
    thread::scope(|scope| {
        scope.spawn(move || assert!(selected.release().is_ok()));
        assert!(gate.acquire(true, Some(Duration::from_secs(5))));
    });
    assert_eq!(ctx.call(release, vec![], vec![]).unwrap(), NONE);
    let error = ctx.call(release, vec![], vec![]).unwrap_err();
    assert_eq!(error.kind, "RuntimeError");
    assert_eq!(error.message, "release unlocked lock");
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
                ctx.heap.execution.pending = Some(error);
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
            let call = if forwarded {
                let method = ctx.bind_python_method(target, value);
                let wrapper =
                    crate::dynamic::method_decorator::static_method(&mut ctx, method).unwrap();
                ctx.prepare_call_in_scope(
                    scope,
                    operator,
                    vec![wrapper].into(),
                    vec![("alias".into(), value)],
                )
                .unwrap()
            } else {
                ctx.prepare_call(target, vec![value].into(), vec![("alias".into(), value)])
                    .unwrap()
            };
            let crate::dynamic::calls::RegisteredCall::Native(frame) = call else {
                panic!("registered function ran during preparation");
            };
            // A parked native entry leaves the complete binding layout with its code.
            let definition = &ctx.code_object(code).definition;
            let layout = definition.parsed_parameters.get().unwrap();
            assert_eq!(Arc::strong_count(layout), 1);
            (value, frame)
        };
        // SAFETY: preparation released its RefMut. This thread owns the stable Context.
        let result = unsafe { frame.run(worker.as_ptr()) };
        let ctx = worker.borrow();
        assert_eq!(ctx.heap.python_depth(), 0);
        assert_eq!(ctx.heap.execution.call_depth, 0);
        assert!(ctx.heap.execution.pending.is_none());
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
    ctx.heap.workers.entries.push(entry);
    let started = ready.wait()?;
    let identity = (started.ident, started.native_id);
    // SAFETY: the test owns the creating context through publication.
    unsafe { started.publish(ctx) }?;
    Ok(identity)
}

fn registered(handle: &Arc<Handle>, daemon: bool, owner: Id) -> Registration {
    Registration::Thread {
        handle: Arc::clone(handle),
        handle_id: NONE,
        thread: RegisteredThread {
            daemon,
            owner,
            run: RunMode::Default,
        },
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
                wait: Arc::clone(lock),
                release: None,
            },
            registered(&handle, false, owner),
            Vec::new(),
        )
        .unwrap();
        handle.join(Some(Duration::ZERO)).unwrap();
        assert!(status(&mut ctx, &handle, Status::Alive));
        assert!(ctx.heap.workers.entries[0].handle.awaiting_exit());
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
    assert!(!ctx.heap.workers.entries[0].handle.awaiting_exit());
    assert!(!status(&mut ctx, &handle, Status::Alive));
    assert_eq!(ctx.interpreter.worker_count.load(Ordering::Acquire), 0);
}

#[test]
fn startup_wrapper_is_a_temporary_of_the_registered_caller() {
    use crate::dynamic::value::{Body, EMPTY_TUPLE, FrameOwner};

    unsafe extern "C" fn run(_: *mut Context, _: Id) -> Id {
        NONE
    }

    let mut ctx = context();
    let scope = ctx.scope("thread_test", None);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));
    let setup = ctx.heap.temporary_mark();
    let name = ctx.text("run");
    let signature = ctx.text(r#"{"positional":["self"],"kwonly":[]}"#);
    let filename = ctx.text("thread_test.py");
    let code = ctx.prelinked_code(
        Body::Native(run, NONE),
        scope,
        name,
        signature,
        (name, NONE),
        (filename, 1),
    );
    let defaults = ctx.dict(Vec::new());
    let method = ctx
        .define_function(code, scope, name, defaults, name, NONE, NONE, false)
        .unwrap();
    let namespace = ctx.dict(vec![(name, method)]);
    let class_name = ctx.text("Thread");
    let metaclass = ctx.builtin("type");
    let class = ctx
        .call(metaclass, vec![class_name, EMPTY_TUPLE, namespace], vec![])
        .unwrap();
    let owner = ctx.instance(class, None);
    ctx.register_thread_run(class, owner);
    let handle = Arc::<Handle>::default();
    let wrapper = ctx.alloc(Value::ThreadHandle(Arc::clone(&handle)));
    let watch = ctx.heap.native_weak_reference(wrapper).unwrap();
    let start = ctx.builtin("_imre_thread.start");
    let kwargs = ctx.dict(Vec::new());
    for (name, value) in [
        ("_target", NONE),
        ("_args", EMPTY_TUPLE),
        ("_kwargs", kwargs),
        ("_context", NONE),
        ("_handle", NONE),
        ("_os_thread_handle", wrapper),
    ] {
        ctx.set_attr(owner, name, value).unwrap();
    }
    // Refuse unsupported functions, descriptors, and attribute hooks before consuming
    // the handle. The same receiver must remain startable after its class is restored.
    let other = ctx
        .define_function(code, scope, name, defaults, name, NONE, NONE, false)
        .unwrap();
    let property = ctx.builtin("property");
    let descriptor = ctx.call(property, vec![method], vec![]).unwrap();
    for replacement in [other, descriptor] {
        ctx.set_attr(class, "run", replacement).unwrap();
        assert_eq!(
            prepare_start(&mut ctx, &[owner, super::super::FALSE], &[])
                .err()
                .unwrap()
                .kind,
            "NotImplementedError"
        );
        handle.check_new().unwrap();
        assert_eq!(ctx.raw_instance_field(owner, "_handle"), Some(NONE));
        assert_eq!(ctx.raw_instance_field(owner, "_target"), Some(NONE));
    }
    ctx.set_attr(class, "run", method).unwrap();
    ctx.set_attr(class, "__getattribute__", other).unwrap();
    assert_eq!(
        prepare_start(&mut ctx, &[owner, super::super::FALSE], &[])
            .err()
            .unwrap()
            .kind,
        "NotImplementedError"
    );
    handle.check_new().unwrap();
    ctx.delete_attr(class, "__getattribute__").unwrap();
    ctx.bind(scope, "receiver", owner);
    ctx.bind(scope, "watch", watch);
    ctx.unwind_operands(setup);

    let mark = ctx.heap.temporary_mark();
    let call = ffi::OperandCall::prepare(&mut ctx, mark, start, |ctx| {
        ctx.prepare_call(start, vec![owner, super::super::FALSE].into(), vec![])
    })
    .unwrap();
    // SAFETY: the ordinary registered start and caller mark belong to this thread.
    let result = unsafe {
        call.run(&raw mut ctx, |ctx, mark, result| {
            assert_eq!(*ctx.heap.temporary_root(mark), wrapper);
            ctx.finish_call_operands(mark, result);
        })
    }
    .unwrap();
    assert!(matches!(ctx.heap.get(result), Value::Tuple(ids) if ids[0] == wrapper));
    assert_eq!(ctx.lookup_attr(owner, "_handle").unwrap(), wrapper);
    assert_ne!(handle.ident(), thread_ident());
    handle.join(Some(Duration::from_secs(5))).unwrap();
    ctx.reap_thread_operands();
    ctx.set_attr(owner, "_os_thread_handle", NONE).unwrap();
    ctx.set_attr(owner, "_handle", NONE).unwrap();
    // Discard the actual startup expression, including its result tuple.
    ctx.unwind_operands(mark);
    ctx.collect_unreachable();
    assert!(matches!(ctx.heap.get(watch), Value::Weak(wr) if wr.target.is_none()));
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
    for (cancel, run) in [
        (true, RunMode::Default),
        (false, RunMode::Default),
        (false, RunMode::Bound),
    ] {
        let owner = ctx.instance(class, None);
        let target = ctx.alloc(Value::List(Vec::new()));
        ctx.set_attr(owner, "_target", target).unwrap();
        let handle = Arc::<Handle>::default();
        let prefix = Arc::<PrimitiveLock>::default();
        let body = Arc::<PrimitiveLock>::default();
        assert!(prefix.acquire(false, None));
        let mut registration = registered(&handle, false, owner);
        if let Registration::Thread { thread, .. } = &mut registration {
            thread.run = run;
        }
        let request = super::Start::new(
            &ctx,
            Operation::Acquire {
                wait: Arc::clone(&body),
                release: Some(prefix.clone()),
            },
            registration,
            vec![target],
        );
        let (ready, entry) = super::launch(request).unwrap();
        ctx.heap.workers.entries.push(entry);
        let started = ready.wait().unwrap();
        assert_ne!(started.ident, thread_ident());
        assert!(ctx.heap.workers.roots().any(|id| *id == owner));
        assert!(prefix.locked());
        assert!(!body.locked());
        let ident = started.ident;
        if cancel {
            drop(started);
        } else {
            // SAFETY: the creating test owns the context and the rooted receiver.
            unsafe { started.publish(&raw mut ctx) }.unwrap();
        }
        assert!(handle.wait(Some(Duration::from_secs(5)), State::worker_pending));
        handle.join(Some(Duration::ZERO)).unwrap();
        ctx.reap_thread_operands();
        assert!(ctx.heap.workers.entries.is_empty());
        assert_eq!(prefix.locked(), cancel);
        assert_eq!(body.locked(), !cancel);
        assert_eq!(handle.ident(), if cancel { 0 } else { ident });
        if cancel || run == RunMode::Bound {
            assert_eq!(ctx.lookup_attr(owner, "_target").unwrap(), target);
        } else {
            assert_eq!(
                ctx.lookup_attr(owner, "_target").unwrap_err().kind,
                "AttributeError"
            );
        }
    }
    ctx.heap.leave(NONE, false);
}

#[cfg(target_os = "linux")]
#[test]
fn python_worker_owns_its_bound_frame_before_publication() {
    use crate::dynamic::value::Body;
    use crate::objects::logging::HandlerLock;
    use std::io::{self, Seek};
    use std::os::fd::AsRawFd;

    fn traceback() -> String {
        let mut file = std::fs::File::from(
            rustix::fs::memfd_create(c"worker-frames", rustix::fs::MemfdFlags::CLOEXEC).unwrap(),
        );
        crate::fault_handler::dump_traceback(file.as_raw_fd(), true);
        file.rewind().unwrap();
        io::read_to_string(file).unwrap()
    }

    unsafe extern "C" fn entry(raw: *mut Context, scope: Id) -> Id {
        // SAFETY: the registered frame owns this context on the worker.
        let ctx = unsafe { &mut *raw };
        ctx.collect_unreachable();
        let result = (|| {
            let receiver = ctx.lookup(scope, "self")?;
            let alias = ctx.lookup(scope, "alias")?;
            if receiver != alias {
                return Err(Error::new("AssertionError", "lost bound receiver identity"));
            }
            let Value::HandlerLock(lock) = ctx.heap.get(receiver) else {
                return Err(Error::new("AssertionError", "lost bound lock"));
            };
            if !lock.acquire(false, None).map_err(Error::from)? {
                return Err(Error::new("ValueError", "worker lock is held"));
            }
            Ok(receiver)
        })();
        result.unwrap_or_else(|error| {
            ctx.heap.execution.pending = Some(error);
            NONE
        })
    }

    let mut ctx = context();
    let interpreter = Arc::clone(&ctx.interpreter);
    let _publication = crate::fault_handler::FramePublication::enter(&interpreter.finalizing);
    let scope = ctx.scope("worker_test", None);
    let name = ctx.text("target");
    let signature = ctx.text(r#"{"positional":["self"],"kwonly":["alias"],"scope_complete":true,"thread_lock_only":true}"#);
    let file = ctx.text("worker_test.py");
    let code = ctx.prelinked_code(
        Body::Native(entry, NONE),
        scope,
        name,
        signature,
        (name, NONE),
        (file, 7),
    );
    let defaults = ctx.dict(Vec::new());
    let function = ctx
        .define_function(code, scope, name, defaults, name, NONE, NONE, false)
        .unwrap();
    let object = ctx.builtin("object");
    let owner = ctx.instance(object, None);
    let args = ctx.alloc(Value::Tuple(Vec::new()));
    let keyword = ctx.text("alias");
    for (cancel, held) in [(true, false), (false, false), (false, true)] {
        let lock = Arc::<HandlerLock>::default();
        if held {
            assert!(matches!(lock.acquire(false, None), Ok(true)));
        }
        let receiver = ctx.alloc(Value::HandlerLock(Arc::clone(&lock)));
        let method = ctx.bind_python_method(function, receiver);
        let kwargs = ctx.dict(vec![(keyword, receiver)]);
        let (operation, roots) = worker_target(&ctx, method, args, kwargs).unwrap();
        assert!(matches!(operation, Operation::Python(_)));
        assert_eq!(roots, [method, args, receiver]);
        let handle = Arc::<Handle>::default();
        let request = Start::new(&ctx, operation, registered(&handle, false, owner), roots);
        let (ready, registration) = super::launch(request).unwrap();
        ctx.heap.workers.entries.push(registration);
        let started = ready.wait().unwrap();
        let ident = started.ident;
        let creator_lock = held.then(|| (thread_ident(), 1));
        assert_ne!(ident, thread_ident());
        assert_eq!(handle.ident(), 0);
        assert_eq!(lock.state(), creator_lock);
        let trace = traceback();
        let header = format!("Thread 0x{ident:0width$x}", width = 2 * size_of::<usize>());
        let frame = r#"  File "worker_test.py", line 7 in target"#;
        assert!(
            trace
                .split("\n\n")
                .any(|stack| stack.starts_with(&header) && stack.lines().any(|line| line == frame)),
            "{trace}"
        );
        if cancel {
            drop(started);
        } else {
            // SAFETY: the creator owns the registration and this context.
            unsafe { started.publish(&raw mut ctx) }.unwrap();
        }
        assert!(handle.wait(Some(Duration::from_secs(5)), State::worker_pending));
        let result = handle.join(Some(Duration::ZERO));
        if held {
            let error = result.unwrap_err();
            assert_eq!(error.kind, "SystemError");
            assert!(error.message.contains("ValueError: worker lock is held"));
        } else {
            result.unwrap();
        }
        assert_eq!(
            lock.state(),
            if cancel || held {
                creator_lock
            } else {
                Some((ident, 1))
            }
        );
        assert!(!traceback().contains(frame));
        assert_eq!(Arc::strong_count(&lock), 2); // The creator's wrapper and test observer.
        ctx.reap_thread_operands();
        assert!(ctx.heap.workers.entries.is_empty());
    }
}

#[test]
fn failed_startup_release_skips_the_blocking_acquire() {
    let mut ctx = context();
    let object = ctx.builtin("object");
    let owner = ctx.instance(object, None);
    let handle = Arc::<Handle>::default();
    let body = Arc::<PrimitiveLock>::default();
    let error = launch(
        &mut ctx,
        Operation::Acquire {
            wait: Arc::clone(&body),
            release: Some(Arc::default()),
        },
        registered(&handle, false, owner),
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.kind, "RuntimeError");
    assert_eq!(error.message, "RuntimeError: release unlocked lock");
    handle.join(Some(Duration::from_secs(2))).unwrap();
    assert!(status(&mut ctx, &handle, Status::IsDone));
    assert!(!body.locked());
    assert_eq!(ctx.interpreter.worker_count.load(Ordering::Acquire), 0);
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
    assert!(status(&mut ctx, &handle, Status::Alive));
    assert_ne!(ident, thread_ident());
    assert!(ctx.check_thread_reporter_mutable().is_ok());

    drop(held);
    handle.join(None).unwrap();
    ctx.reap_thread_operands();
    assert!(ctx.heap.workers.entries.is_empty());
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
    assert!(!ctx.heap.workers.has_unreaped_workers());
    let mark = ctx.heap.temporary_mark();
    let object = ctx.builtin("object");
    let operand = ctx.instance(object, None);
    let weak = ctx.heap.native_weak_reference(operand).unwrap();

    // External roots can be enumerated before the worker is registered.
    let globals: Vec<_> = ctx.heap_and_roots().1.collect();
    let active = while_lock_held(|gate| {
        let start = prepare_detached(
            &mut ctx,
            Operation::Acquire {
                wait: Arc::clone(gate),
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
        assert!(ctx.heap.workers.has_unreaped_workers());
        assert!(ctx.heap.workers.shutdown_handles().next().is_none());
        assert!(ctx.heap.workers.live_threads().next().is_none());
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
        assert!(!status(&mut ctx, &joined, Status::Alive));
        ctx.reap_thread_operands();
        assert!(!ctx.heap.workers.roots().any(|&id| id == owner));
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(weak);
        ctx.heap.collect(globals.iter().copied());
        assert!(matches!(ctx.heap.get(weak), Value::Weak(wr) if wr.target == Some(operand)));
        Arc::clone(&ctx.heap.workers.entries[0].handle)
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while active.awaiting_exit() {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(ctx.check_thread_reporter_mutable().is_ok());
    // A low-level worker can exit between reaping and enumerating GC roots.
    ctx.reap_thread_operands();
    assert!(ctx.heap.workers.has_unreaped_workers());
    assert!(ctx.heap.workers.roots().any(|&id| id == operand));
    ctx.collect_unreachable();
    assert!(!ctx.heap.workers.has_unreaped_workers());
    assert!(matches!(ctx.heap.get(weak), Value::Weak(wr) if wr.target.is_none()));
}

#[test]
fn zero_timeout_applies_to_worker_exit_and_all_observers_share_native_join() {
    let mut ctx = context();
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
    assert!(status(&mut ctx, &handle, Status::Alive));
    end_body.send(()).unwrap();
    at_exit.recv_timeout(timeout).unwrap();
    thread::scope(|scope| {
        let observer = Arc::clone(&handle);
        scope.spawn(move || assert!(!status(&mut context(), &observer, Status::Alive)));
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
fn contended_condition_restore_parks_its_caller_and_recovers_recursion() {
    use crate::objects::logging::HandlerLock;
    use std::sync::atomic::AtomicUsize;

    let mut ctx = context();
    let parked = Arc::<AtomicUsize>::default();
    ctx.heap.parked_count_probe = Some(Arc::clone(&parked));
    let lock = Arc::<HandlerLock>::default();
    let receiver = ctx.alloc(Value::HandlerLock(Arc::clone(&lock)));
    let restore = ctx.lookup_attr(receiver, "_acquire_restore").unwrap();
    let save = ctx.lookup_attr(receiver, "_release_save").unwrap();
    assert_eq!(lock.acquire(true, None).ok(), Some(true));
    assert_eq!(lock.acquire(true, None).ok(), Some(true));
    let state = ctx.call(save, vec![], vec![]).unwrap();
    let owner = thread_ident();
    let timeout = Duration::from_secs(3);
    let observed = thread::scope(|scope| {
        let (ready, acquired) = mpsc::sync_channel(0);
        let lock = Arc::clone(&lock);
        let parked = Arc::clone(&parked);
        let peer = scope.spawn(move || {
            assert_eq!(lock.acquire(true, Some(timeout)).ok(), Some(true));
            ready.send(()).unwrap();
            let deadline = std::time::Instant::now() + timeout;
            while !lock.has_parked_waiter() && std::time::Instant::now() < deadline {
                thread::yield_now();
            }
            let observation = (lock.has_parked_waiter(), parked.load(Ordering::Acquire));
            assert!(lock.release().is_ok());
            observation
        });
        acquired.recv_timeout(timeout).unwrap();
        assert_eq!(ctx.call(restore, vec![state], vec![]).unwrap(), NONE);
        peer.join().unwrap()
    });
    assert_eq!(observed, (true, 1));
    assert_eq!(parked.load(Ordering::Acquire), 0);
    assert_eq!(lock.state(), Some((owner, 2)));
}

#[test]
fn registered_observers_park_through_native_tls_teardown() {
    use std::sync::atomic::AtomicUsize;

    struct ExitGate {
        handle: Arc<Handle>,
        parked: Arc<AtomicUsize>,
        entered: Option<mpsc::SyncSender<()>>,
        observed: mpsc::Sender<usize>,
    }
    impl Drop for ExitGate {
        fn drop(&mut self) {
            if let Some(entered) = &self.entered {
                let _ = entered.send(());
            }
            // The body has signaled exit. Sample the owning heap only once the caller
            // claims the native join that cannot finish until this destructor returns.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while self.handle.native.try_lock().is_ok() {
                if std::time::Instant::now() >= deadline {
                    return;
                }
                thread::yield_now();
            }
            let _ = self.observed.send(self.parked.load(Ordering::Acquire));
        }
    }
    thread_local! {
        static GATE: std::cell::OnceCell<ExitGate> = const { std::cell::OnceCell::new() };
    }

    let mut ctx = context();
    let parked = Arc::<AtomicUsize>::default();
    ctx.heap.parked_count_probe = Some(Arc::clone(&parked));
    let timeout = Duration::from_secs(5);
    let mut depths = Vec::new();
    for name in ["join", "is_done", "_imre_thread.alive", "_thread._shutdown"] {
        let shutdown = name == "_thread._shutdown";
        let handle = Arc::<Handle>::default();
        let receiver = ctx.alloc(Value::ThreadHandle(Arc::clone(&handle)));
        let (callable, args, expected) = match name {
            "join" => (
                ctx.lookup_attr(receiver, name).unwrap(),
                vec![ctx.int(0)],
                NONE,
            ),
            "is_done" => (
                ctx.lookup_attr(receiver, name).unwrap(),
                vec![],
                boolean(true),
            ),
            "_imre_thread.alive" => (ctx.builtin(name), vec![receiver], boolean(false)),
            _ => {
                let object = ctx.builtin("object");
                let owner = ctx.instance(object, None);
                ctx.heap.workers.entries.push(Entry {
                    thread: Some(RegisteredThread {
                        owner,
                        daemon: false,
                        run: RunMode::Default,
                    }),
                    handle: Arc::clone(&handle),
                    operands: vec![],
                });
                (ctx.builtin(name), vec![], NONE)
            }
        };
        let (entered, at_exit) = mpsc::sync_channel(0);
        let (observed, observation) = mpsc::channel();
        let gate = ExitGate {
            handle: Arc::clone(&handle),
            parked: Arc::clone(&parked),
            entered: (!shutdown).then_some(entered),
            observed,
        };
        handle
            .spawn(move |_| {
                gate.handle.ident.set(Some(thread_ident())).unwrap();
                // Shutdown selects live workers. Exit only after this caller parks to wait.
                if shutdown {
                    let deadline = std::time::Instant::now() + timeout;
                    while gate.parked.load(Ordering::Acquire) == 0 {
                        if std::time::Instant::now() >= deadline {
                            return Err("shutdown did not park".into());
                        }
                        thread::yield_now();
                    }
                }
                GATE.with(|cell| assert!(cell.set(gate).is_ok()));
                Ok(())
            })
            .unwrap();
        if !shutdown {
            at_exit.recv_timeout(timeout).unwrap();
        }
        let call = ctx.prepare_call(callable, args.into(), vec![]).unwrap();
        assert!(
            matches!(observation.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "{name} joined the native thread during preparation"
        );
        // SAFETY: preparation has returned and the observing caller is still active.
        let observed = unsafe { call.run(&raw mut ctx) }.unwrap();
        assert_eq!(observed, expected, "{name}");
        assert_eq!(parked.load(Ordering::Acquire), 0, "{name} returned parked");
        depths.push(
            observation
                .try_recv()
                .expect("observer returned before native TLS teardown"),
        );
    }
    assert_eq!(
        depths,
        [1, 1, 1, 1],
        "join(0), is_done(), is_alive(), _shutdown()"
    );
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
                wait: Arc::clone(lock),
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
        assert!(daemon.awaiting_exit());
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
    assert!(!status(&mut ctx, &handle, Status::IsDone));
    assert!(handle.set_done().is_err());
    let active = while_lock_held(|gate| {
        let (ident, _) = launch(
            &mut ctx,
            Operation::Acquire {
                wait: Arc::clone(gate),
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
        assert!(status(&mut ctx, &handle, Status::IsDone));
        assert!(handle.awaiting_exit());
        ctx.reap_thread_operands();
        assert_eq!(ctx.heap.workers.entries.len(), 1);
        Arc::clone(&ctx.heap.workers.entries[0].handle)
    });
    let shutdown = ctx.builtin("_thread._shutdown");
    assert_eq!(ctx.call(shutdown, vec![], vec![]).unwrap(), NONE);
    assert!(!active.awaiting_exit());
    assert!(!handle.awaiting_exit());
    assert!(ctx.heap.workers.entries.is_empty());
}

#[test]
fn set_done_after_body_exit_releases_the_unclaimed_native_handle() {
    let mut ctx = context();
    let handle = Arc::<Handle>::default();
    handle.spawn(|_| Ok(())).unwrap();
    assert!(handle.wait(Some(Duration::from_secs(5)), State::join_pending));
    // The body signalled exit, but no Python observer has joined its OS thread.
    assert!(handle.native.lock().unwrap().thread.is_some());
    handle.set_done().unwrap();
    assert!(handle.native.lock().unwrap().thread.is_none());
    assert!(status(&mut ctx, &handle, Status::IsDone));
    handle.join(None).unwrap();
}
