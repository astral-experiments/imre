use super::super::{FrameOwner, Id, NONE, Value};
use crate::dynamic::{ffi, stdlib, weakref};

#[test]
fn retained_closure_owner_rechecks_the_cell_and_its_contents() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let scope = ctx.scope("sample", None);
    let owner = ctx.alloc(Value::List(Vec::new()));
    let cell = ctx.alloc(Value::Cell(Some(owner)));
    let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    frame.closure.cells.insert("held".into(), cell);

    let roots = [scope, NONE];
    let paths =
        super::RootPaths::discover(&ctx.heap, roots, [owner].into_iter(), true, false).unwrap();
    assert!(paths.contains(owner) && paths.valid(&ctx.heap, roots));

    *ctx.heap.get_mut(cell) = Value::Cell(None);
    assert!(!paths.valid(&ctx.heap, roots));
    *ctx.heap.get_mut(cell) = Value::Cell(Some(owner));
    let replacement = ctx.alloc(Value::Cell(Some(owner)));
    let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    frame.closure.cells.insert("held".into(), replacement);
    assert!(!paths.valid(&ctx.heap, roots));
}

#[test]
fn cached_operand_rechecks_the_frame_and_its_current_edge() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let class = ctx.builtin("object");
    let owner = ctx.instance(class, None);
    let other = ctx.instance(class, None);
    let iterator = ctx.alloc(Value::LoopIterator {
        source: owner,
        peeked: None,
    });
    let scope = ctx.scope("sample", None);
    let heap = &mut ctx.heap;
    heap.release_temporaries_since(0);
    heap.enter(scope, NONE, FrameOwner::Module(NONE));
    heap.keep(iterator);
    assert!(heap.operand_keeps_finalizer(owner, 1));
    assert!(!heap.operand_keeps_finalizer(owner, 0));

    heap.enter(scope, NONE, FrameOwner::Module(NONE));
    assert!(heap.operand_keeps_finalizer(owner, 0));
    let Value::LoopIterator { source, .. } = heap.get_mut(iterator) else {
        unreachable!()
    };
    *source = other;
    assert!(!heap.operand_keeps_finalizer(owner, 0));
    assert!(heap.operand_keeps_finalizer(other, 0));

    heap.leave(NONE, false);
    assert!(!heap.operand_keeps_finalizer(other, 0));
    heap.leave(NONE, false);
    assert!(!heap.operand_keeps_finalizer(other, usize::MAX));
}

#[test]
fn a_bound_method_keeps_its_receiver_alive_until_the_binding_is_removed() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let scope = ctx.scope("sample", None);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));
    let class = ctx.builtin("object");
    let receiver = ctx.instance(class, None);
    ctx.heap.register_finalizer(receiver);
    let callable = ctx.builtin("repr");
    let method = ctx.alloc(Value::Method {
        callable,
        receiver,
        explicit: false,
    });
    let globals = ctx.finalizer_path_roots();
    let heap = &mut ctx.heap;
    let Value::Scope(bindings) = heap.get_mut(scope) else {
        unreachable!()
    };
    bindings.names.insert("callback".into(), method);
    heap.release_temporaries_since(0);

    heap.learn_finalizer_paths(globals, None);
    assert!(heap.finalizer_roots.contains(receiver));
    assert!(heap.finalizer_roots.valid(heap, globals));

    let Value::Scope(bindings) = heap.get_mut(scope) else {
        unreachable!()
    };
    bindings.names.clear();
    assert!(!heap.finalizer_roots.valid(heap, globals));
}

#[test]
fn finalizer_paths_recheck_scope_owners_and_their_weak_references() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let class = ctx.builtin("object");
    let mark = ctx.heap.temporary_mark();
    let scope = ctx.scope("sample", None);
    ctx.heap.release_temporaries_since(mark);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));

    let mut owners = Vec::new();
    let mut references = Vec::new();
    for _ in 0..80 {
        let owner = ctx.instance(class, None);
        ctx.heap.register_finalizer(owner);
        owners.push(owner);
        references.push(
            ctx.heap
                .new_weak_reference(owner, NONE, weakref::Kind::Reference, None)
                .unwrap(),
        );
    }
    let held = ctx.alloc(Value::List(owners));
    let refs = ctx.alloc(Value::List(references));
    let globals = ctx.finalizer_path_roots();
    let streams = stdlib::io::rooted_standard_streams(&ctx);
    let heap = &mut ctx.heap;
    let Value::Scope(bindings) = heap.get_mut(scope) else {
        unreachable!()
    };
    bindings.names.insert("held".into(), held);
    bindings.names.insert("refs".into(), refs);
    heap.release_temporaries_since(0);

    heap.learn_finalizer_paths(globals, streams.as_ref());
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), None));
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));

    for (list, before_discard) in [(refs, Some(0)), (held, None)] {
        let Value::List(items) = heap.get_mut(list) else {
            unreachable!()
        };
        let old = std::mem::replace(&mut items[0], NONE);
        assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), before_discard));
        let Value::List(items) = heap.get_mut(list) else {
            unreachable!()
        };
        items[0] = old;
    }
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));

    let extra = ctx.instance(class, None);
    let heap = &mut ctx.heap;
    heap.register_finalizer(extra);
    heap.release_temporaries_since(0);
    assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), None));
    heap.keep(extra);
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), None));
    assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));
    heap.release_temporaries_since(0);
    heap.take_finalizer(extra, false);
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));

    heap.leave(NONE, false);
    assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), None));
    assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));
}

#[test]
fn published_loop_sources_and_peeked_items_keep_finalizers_alive() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let class = ctx.builtin("object");
    let scope = ctx.scope("sample", None);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));
    let source = ctx.instance(class, None);
    let item = ctx.instance(class, None);
    let cursor = ctx.alloc(Value::LoopIterator {
        source,
        peeked: Some(item),
    });
    let globals = ctx.finalizer_path_roots();
    let streams = stdlib::io::rooted_standard_streams(&ctx);
    let heap = &mut ctx.heap;
    heap.register_finalizer(source);
    heap.register_finalizer(item);
    heap.release_temporaries_since(0);
    heap.keep(cursor);
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), Some(1)));
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), None));
    assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));

    for _ in 0..32 {
        heap.enter(scope, NONE, FrameOwner::Module(NONE));
    }
    assert!(heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));
    let Value::LoopIterator { peeked, .. } = heap.get_mut(cursor) else {
        unreachable!();
    };
    *peeked = None;
    assert!(!heap.cached_finalizers_kept(globals, streams.as_ref(), Some(0)));
}

#[test]
fn search_keeps_proven_paths_when_the_node_budget_runs_out() {
    use super::{Heap, Root, RootPaths};
    use std::collections::HashSet;

    let mut heap = Heap::new();
    let first = heap.alloc(Value::List(Vec::new()));
    let second = heap.alloc(Value::List(Vec::new()));
    let root = heap.alloc(Value::List(vec![first, second]));
    let globals = [root, NONE];
    let mut wanted = HashSet::from([first, second]);
    let mut paths = RootPaths::default();
    let mut nodes_left = 2; // Enough for the root and its first child.
    let mut edges_left = 32;
    paths
        .search(
            &heap,
            [(root, Root::Global(0))].into_iter(),
            &mut wanted,
            &mut nodes_left,
            &mut edges_left,
            super::Search::Indexed,
        )
        .unwrap();

    assert_eq!(nodes_left, 0);
    assert_eq!(wanted, HashSet::from([second]));
    assert!(paths.contains(first));
    assert!(!paths.contains(second));

    // The two paths share a root across successive searches.
    nodes_left = 3;
    paths
        .search(
            &heap,
            [(root, Root::Global(0))].into_iter(),
            &mut wanted,
            &mut nodes_left,
            &mut edges_left,
            super::Search::Indexed,
        )
        .unwrap();
    assert!(wanted.is_empty());
    assert!(paths.contains(first));
    assert!(paths.contains(second));
    assert_eq!(paths.nodes.len(), 3);
    assert!(paths.valid(&heap, globals));
    let Value::List(items) = heap.get_mut(root) else {
        unreachable!()
    };
    items.remove(0);
    assert!(!paths.valid(&heap, globals));
}

#[test]
fn a_frame_can_hold_a_weak_target_without_a_finalizer_path() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let mark = ctx.heap.temporary_mark();
    let scope = ctx.scope("sample", None);
    ctx.heap.release_temporaries_since(mark);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));
    let target = ctx.alloc(Value::List(Vec::new()));
    let reference = ctx
        .heap
        .new_weak_reference(target, NONE, weakref::Kind::Reference, None)
        .unwrap();
    let registry = ctx.alloc(Value::List(vec![reference]));
    ctx.bind(scope, "target", target);
    ctx.heap.release_temporaries_since(0);

    ctx.heap.learn_weak_roots(registry, NONE);
    assert!(ctx.heap.weak_values_kept_by(registry, NONE));
    ctx.heap.leave(NONE, false);
    assert!(!ctx.heap.weak_values_kept_by(registry, NONE));
    ctx.heap.collect([registry].into_iter());
    assert!(
        ctx.heap
            .get(reference)
            .weak_reference()
            .unwrap()
            .target
            .is_none()
    );
}

#[test]
fn weak_dict_key_and_frame_held_target_must_both_survive() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let registry = ctx.dict(Vec::new());
    let mark = ctx.heap.temporary_mark();
    let scope = ctx.scope("sample", None);
    ctx.heap.release_temporaries_since(mark);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));

    let class = ctx.builtin("object");
    let target = ctx.instance(class, None);
    ctx.bind(scope, "result", target);
    let callback = ctx.builtin("print");
    let reference = ctx
        .heap
        .new_weak_reference(target, callback, weakref::Kind::Reference, None)
        .unwrap();
    ctx.put_item(registry, reference, NONE).unwrap();
    ctx.heap.release_temporaries_since(0);

    ctx.heap.learn_weak_roots(registry, NONE);
    assert!(!ctx.heap.weak_values_kept_by(registry, NONE));
    for _ in 0..=super::SEARCH_BACKOFF {
        ctx.heap.learn_weak_roots(registry, NONE);
    }
    assert!(ctx.heap.weak_values_kept_by(registry, NONE));

    ctx.delete_item(registry, reference).unwrap();
    assert!(!ctx.heap.weak_values_kept_by(registry, NONE));
    ctx.put_item(registry, NONE, NONE).unwrap();
    assert!(!ctx.heap.weak_values_kept_by(registry, NONE));
    ctx.delete_item(registry, NONE).unwrap();
    ctx.put_item(registry, reference, NONE).unwrap();
    assert!(ctx.heap.weak_values_kept_by(registry, NONE));

    ctx.bind(scope, "saved_result", target);
    ctx.bind(scope, "result", NONE);
    assert!(!ctx.heap.weak_values_kept_by(registry, NONE));
    ctx.heap.learn_weak_roots(registry, NONE);
    assert!(ctx.heap.weak_values_kept_by(registry, NONE));

    ctx.heap.leave(NONE, false);
    assert!(!ctx.heap.weak_values_kept_by(registry, NONE));
    assert_eq!(
        ctx.heap.collect([registry].into_iter()),
        Some(vec![(callback, reference)])
    );
}

#[test]
fn wide_siblings_leave_room_for_nested_roots_and_late_edges() {
    use super::{MAX_NODES, RootPaths};

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let deep = ctx.alloc(Value::List(Vec::new()));
    let nested = ctx.alloc(Value::List(vec![deep]));
    let items: Vec<_> = (0..MAX_NODES)
        .map(|_| ctx.alloc(Value::List(Vec::new())))
        .collect();
    let late = items[MAX_NODES / 4];
    let wide = ctx.alloc(Value::List(items));
    let root = ctx.alloc(Value::List(vec![wide, nested]));
    let globals = [root, NONE];
    let paths =
        RootPaths::discover(&ctx.heap, globals, [deep, late].into_iter(), false, false).unwrap();
    assert!(paths.contains(deep) && paths.contains(late));
    assert!(paths.valid(&ctx.heap, globals));

    let Value::List(items) = ctx.heap.get_mut(wide) else {
        unreachable!()
    };
    items[MAX_NODES / 4] = NONE;
    assert!(!paths.valid(&ctx.heap, globals));
}

#[test]
fn a_wide_frame_probe_leaves_room_to_follow_global_modules() {
    use super::{MAX_NODES, RootPaths};

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let module = ctx.allocate_module(NONE);
    let target = ctx.alloc(Value::List(Vec::new()));
    let Value::Module { scope: globals, .. } = *ctx.heap.get(module) else {
        unreachable!()
    };
    let Value::Scope(global_scope) = ctx.heap.get_mut(globals) else {
        unreachable!()
    };
    global_scope.names.insert("target".into(), target);

    let items = (0..MAX_NODES)
        .map(|_| ctx.alloc(Value::List(Vec::new())))
        .collect();
    let wide = ctx.alloc(Value::List(items));
    let scope = ctx.scope("caller", None);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));
    let Value::Scope(caller) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    caller.names.insert("wide".into(), wide);
    let globals = [module, NONE];
    let paths =
        RootPaths::discover(&ctx.heap, globals, [target].into_iter(), false, false).unwrap();
    assert!(paths.covers_endpoints);
    assert!(paths.valid(&ctx.heap, globals));
}

#[test]
fn wide_frame_contents_do_not_hide_direct_global_roots() {
    use super::{MAX_NODES, RootPaths};

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let target = ctx.alloc(Value::List(Vec::new()));
    let reference = ctx.alloc(Value::List(Vec::new()));
    let registry = ctx.alloc(Value::List(vec![target]));
    let globals = [registry, reference];
    let items = (0..MAX_NODES)
        .map(|_| ctx.alloc(Value::List(Vec::new())))
        .collect();
    let wide = ctx.alloc(Value::List(items));
    let scope = ctx.scope("caller", None);
    ctx.heap.enter(scope, NONE, FrameOwner::Module(NONE));
    let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    frame.names.insert("wide".into(), wide);
    ctx.heap.release_temporaries_since(0);

    let paths = RootPaths::discover(
        &ctx.heap,
        globals,
        [target, reference].into_iter(),
        true,
        false,
    )
    .unwrap();
    assert!(paths.contains(target) && paths.contains(reference));
    assert!(paths.valid(&ctx.heap, globals));
    let Value::List(items) = ctx.heap.get_mut(registry) else {
        unreachable!()
    };
    items.clear();
    assert!(!paths.valid(&ctx.heap, globals));
}

#[test]
fn late_dictionary_values_keep_the_full_ordinary_search_budget() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let class = ctx.builtin("object");
    let target = ctx.instance(class, None);
    let reference = ctx
        .heap
        .new_weak_reference(target, NONE, weakref::Kind::Reference, None)
        .unwrap();
    let mut entries: Vec<_> = (0..36_000).map(|n| (ctx.int(n), NONE)).collect();
    entries.push((ctx.int(-1), target));
    entries.push((ctx.int(-2), reference));
    let registry = ctx.dict(entries);
    ctx.heap.release_temporaries_since(0);

    ctx.heap.learn_weak_roots(registry, NONE);
    assert!(ctx.heap.cached_weak_roots([registry, NONE]));
}

fn assert_traced_edges(heap: &super::Heap, paths: &super::RootPaths) {
    for &(parent, _, child) in &paths.edges {
        let mut visit = super::super::FindReferences {
            targets: [child; 2],
            found: false,
        };
        heap.get(parent).try_children(&mut visit).unwrap();
        assert!(visit.found, "untraced edge {parent} -> {child}");
    }
}

#[test]
fn a_slotted_owner_is_kept_only_while_the_member_holds_it() {
    use super::RootPaths;

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let class = ctx.builtin("object");
    let holder = ctx.instance(class, None);
    let owner = ctx.instance(class, None);
    let name = ctx.text("resource");
    let member = ctx.alloc(Value::SlotMember {
        owner: class,
        name,
        storage: super::super::SlotStorage::Instance,
    });
    ctx.write_slot_member(member, holder, Some(owner)).unwrap();
    let roots = [holder, NONE];
    let paths = RootPaths::discover(&ctx.heap, roots, [owner].into_iter(), false, false).unwrap();
    assert!(paths.covers_endpoints);
    assert!(paths.valid(&ctx.heap, roots));
    assert_traced_edges(&ctx.heap, &paths);

    ctx.write_slot_member(member, holder, Some(NONE)).unwrap();
    assert!(!paths.valid(&ctx.heap, roots));
    ctx.write_slot_member(member, holder, Some(owner)).unwrap();
    assert!(paths.valid(&ctx.heap, roots));
    ctx.write_slot_member(member, holder, None).unwrap();
    assert!(!paths.valid(&ctx.heap, roots));
}

fn generator_value(scope: Id) -> Value {
    Value::Generator {
        owner: FrameOwner::Module(NONE),
        frame: None,
        name: NONE,
        qualname: NONE,
        lineno: 0,
        code: NONE,
        scope,
        native: None,
        offset: Some(0),
        running: false,
    }
}

#[test]
fn generator_ownership_tracks_live_iterator_inputs_and_scope_bindings() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let scope = ctx.scope("local", None);
    let owner = ctx.alloc(generator_value(NONE));
    let parent = ctx.alloc(generator_value(scope));
    let root = ctx.alloc(Value::Map {
        function: NONE,
        inputs: [parent].into(),
        strict: false,
    });
    ctx.bind(scope, "item", owner);
    let heap = &mut ctx.heap;
    heap.register_finalizer(owner);
    heap.release_temporaries_since(0);
    assert!(heap.short_path_keeps_owner(root, owner));
    assert!(heap.retained_paths_keep_owners([root].into_iter(), true));
    assert!(!heap.retained_paths_keep_owners([].into_iter(), true));

    // Replacing the binding closes a cycle and removes its only path to owner.
    let Value::Scope(locals) = heap.get_mut(scope) else {
        unreachable!()
    };
    locals.names.insert("item".into(), root);
    assert!(!heap.short_path_keeps_owner(root, owner));
    assert!(!heap.retained_paths_keep_owners([root].into_iter(), true));
}

#[test]
fn iterator_paths_recheck_the_suspended_operands() {
    use super::RootPaths;

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let scope = ctx.scope("suspended", None);
    let heap = &mut ctx.heap;
    let target = heap.alloc(generator_value(NONE));
    let generator = heap.alloc(generator_value(scope));
    let cursor = heap.alloc(Value::LoopIterator {
        source: NONE,
        peeked: Some(target),
    });
    let source = ctx.dict(vec![(NONE, cursor)]);
    ctx.bind(scope, ".0", source);
    let heap = &mut ctx.heap;
    let coroutine = heap.alloc(Value::Coroutine {
        frame: generator,
        origin: NONE,
    });
    let wrapper = heap.alloc(Value::CoroutineWrapper(coroutine));
    let mapped = heap.alloc(Value::Map {
        function: NONE,
        inputs: [wrapper].into(),
        strict: false,
    });
    let zipped = heap.alloc(Value::Zip {
        inputs: [mapped].into(),
        strict: false,
    });
    let globals = [zipped, NONE];
    let paths =
        RootPaths::discover(heap, globals, [target, scope].into_iter(), false, false).unwrap();
    assert!(paths.covers_endpoints);
    assert!(paths.valid(heap, globals));
    assert_traced_edges(heap, &paths);

    let Value::LoopIterator { peeked, .. } = heap.get_mut(cursor) else {
        unreachable!()
    };
    *peeked = None;
    assert!(!paths.valid(heap, globals));
    let Value::LoopIterator { peeked, .. } = heap.get_mut(cursor) else {
        unreachable!()
    };
    *peeked = Some(target);
    assert!(paths.valid(heap, globals));

    let Value::Generator { scope: current, .. } = heap.get_mut(generator) else {
        unreachable!()
    };
    *current = NONE;
    assert!(!paths.valid(heap, globals));
    let Value::Generator { scope: current, .. } = heap.get_mut(generator) else {
        unreachable!()
    };
    *current = scope;
    assert!(paths.valid(heap, globals));

    assert!(heap.short_path_keeps_owner(generator, target));
    ctx.delete_item(source, NONE).unwrap();
    assert!(!paths.valid(&ctx.heap, globals));
    assert!(!ctx.heap.short_path_keeps_owner(generator, target));
}

#[test]
fn suspended_cleanup_keeps_exit_methods_and_a_pending_return() {
    use crate::dynamic::native_suspension::{
        Activation, imre_dyn_coro_open, imre_dyn_suspend_operands, imre_dyn_suspend_return,
    };
    use crate::dynamic::{Context, Id, value::Body};
    use std::ffi::c_void;

    unsafe extern "C" fn finish(frame: *mut c_void, destroy: bool) {
        if !destroy {
            // SAFETY: the entry below uses the live activation as this frame's handle.
            unsafe { imre_dyn_suspend_return(frame.cast(), NONE) };
        }
    }

    unsafe extern "C" fn hold(activation: *mut Activation, _scope: Id) -> *mut c_void {
        // SAFETY: the native entry's first word is its current Context (the generated ABI).
        let ctx = unsafe { &mut **activation.cast::<*mut Context>() };
        let class = ctx.builtin("object");
        let callback = ctx.builtin("repr");
        let exits = [0; 2].map(|_| {
            let receiver = ctx.instance(class, None);
            ctx.alloc(Value::Method {
                callable: callback,
                receiver,
                explicit: false,
            })
        });
        let returned = ctx.instance(class, None);
        let targets = ctx.alloc(Value::Tuple(vec![exits[0], exits[1], returned]));
        // The yield operand is for the test to identify the three values. Only the
        // pending return and exit callables remain in the suspended frame.
        ctx.heap.release_temporaries_since(0);
        for value in [exits[0], exits[1], returned] {
            ctx.heap.keep(value);
        }
        // SAFETY: both helpers are called during this activation's running entry.
        unsafe {
            imre_dyn_coro_open(activation, activation.cast(), finish);
            imre_dyn_suspend_operands(activation, targets, ctx.heap.temporary_mark(), 0);
        }
        activation.cast()
    }

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let scope = ctx.scope("sample", None);
    let name = ctx.text("hold");
    let code = ctx.prelinked_code(
        Body::Suspended(hold),
        scope,
        name,
        NONE,
        (name, NONE),
        (name, 1),
    );
    let defaults = ctx.dict(Vec::new());
    let function = ctx
        .define_function(code, scope, name, defaults, name, NONE, NONE, false)
        .unwrap();
    let generator = ctx.new_generator(code, scope, FrameOwner::Function(function));
    let yielded = ctx.next_item(generator).unwrap().unwrap();
    let targets = ctx.items(yielded).unwrap();
    assert_eq!(targets.len(), 3);
    ctx.heap.release_temporaries_since(0);
    let roots = [generator, NONE];
    let paths =
        super::RootPaths::discover(&ctx.heap, roots, targets.into_iter(), false, false).unwrap();
    assert!(paths.covers_endpoints);
    assert!(paths.valid(&ctx.heap, roots));
    assert_traced_edges(&ctx.heap, &paths);

    assert!(ctx.next_item(generator).unwrap().is_none());
    assert!(!paths.valid(&ctx.heap, roots));
}

#[test]
fn standard_output_roots_follow_rebinding_and_deletion() {
    let mut ctx = ffi::new_context(
        serde_json::json!({"paths": [], "python_version": "3.14"}),
        ["imre"],
    );
    let sys = ctx.module("sys").unwrap();
    let roots = stdlib::io::rooted_standard_streams(&ctx).unwrap();
    let [stdout, stderr] = ctx.heap.standard_outputs.map(Option::unwrap);
    assert_eq!(&roots[3..], &[stdout, stderr]);

    ctx.set_attr(sys, "__stdout__", stderr).unwrap();
    ctx.delete_attr(sys, "__stderr__").unwrap();
    let roots = stdlib::io::rooted_standard_streams(&ctx).unwrap();
    assert_eq!(&roots[3..], &[stderr, NONE]);
    assert!(!roots.contains(&stdout));
}

#[test]
fn learns_a_descriptor_added_after_the_ordinary_finalizer_paths() {
    let mut heap = super::Heap::new();
    let ordinary = heap.alloc(Value::List(Vec::new()));
    heap.register_finalizer(ordinary);
    let owners = heap.alloc(Value::List(vec![ordinary]));
    let globals = [owners, NONE];
    heap.release_temporaries_since(0);
    assert!(heap.learn_finalizer_paths(globals, None));

    let Ok(socket) = crate::objects::socket::open(libc::AF_UNIX, libc::SOCK_STREAM, 0) else {
        panic!("could not open a Unix socket");
    };
    // Track the new descriptor independently of the ordinary Python finalizer above.
    let class = heap.alloc(Value::Native("type".into()));
    let descriptor = heap.alloc_slot(Value::Socket {
        class,
        handle: socket,
    });
    heap.track_initialized_descriptor(descriptor).unwrap();
    let Value::List(items) = heap.get_mut(owners) else {
        unreachable!()
    };
    items.push(descriptor);
    heap.release_temporaries_since(0);
    assert!(heap.cached_finalizers_kept(globals, None, None));
    assert!(!heap.cached_finalizers_kept(globals, None, Some(0)));

    for _ in 0..=super::SEARCH_BACKOFF {
        heap.learn_descriptor_paths(globals, None);
    }
    assert!(heap.cached_finalizers_kept(globals, None, Some(0)));
    let Value::List(items) = heap.get_mut(owners) else {
        unreachable!()
    };
    items.pop();
    assert!(!heap.cached_finalizers_kept(globals, None, Some(0)));
}

#[test]
fn rechecks_surviving_operands_after_a_release() {
    let mut heap = super::super::Heap::new();
    let owner = heap.alloc(Value::List(Vec::new()));
    heap.register_finalizer(owner);
    let inner = heap.alloc(Value::List(vec![owner]));
    let outer = heap.alloc(Value::Tuple(vec![inner]));
    heap.release_temporaries_since(0);
    heap.keep(outer);
    assert!(heap.retained_paths_keep_owners([].into_iter(), true));
    heap.release_temporaries_since(0);
    heap.keep(inner);
    assert!(heap.retained_paths_keep_owners([].into_iter(), true));
    assert!(heap.retained_paths_keep_owners([inner].into_iter(), false));
    heap.release_temporaries_since(0);
    assert!(!heap.retained_paths_keep_owners([].into_iter(), true));
}

#[test]
fn finalizer_drain_and_operand_release_require_different_live_owners() {
    let mut heap = super::super::Heap::new();
    let owner = heap.alloc(Value::List(Vec::new()));
    heap.register_finalizer(owner);
    let target = heap.alloc(Value::List(Vec::new()));
    let reference = heap
        .new_weak_reference(target, NONE, weakref::Kind::Reference, None)
        .unwrap();
    let root = heap.alloc(Value::List(vec![owner, target, reference]));
    heap.release_temporaries_since(0);
    assert!(heap.retained_paths_keep_owners([root].into_iter(), true));
    let Value::List(items) = heap.get_mut(root) else {
        unreachable!()
    };
    items.remove(1);
    assert!(heap.retained_paths_keep_owners([root].into_iter(), false));
    assert!(!heap.retained_paths_keep_owners([root].into_iter(), true));
    let Value::List(items) = heap.get_mut(root) else {
        unreachable!()
    };
    items.clear();
    assert!(!heap.retained_paths_keep_owners([root].into_iter(), false));
}

#[test]
fn a_reused_slot_does_not_keep_the_old_path_alive() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let callback = ctx.builtin("print");
    let heap = &mut ctx.heap;
    let owner = heap.alloc(Value::List(Vec::new()));
    heap.register_finalizer(owner);
    let old_edge = heap.alloc(Value::List(vec![owner]));
    let reference = heap
        .new_weak_reference(old_edge, callback, weakref::Kind::Reference, None)
        .unwrap();
    let root = heap.alloc(Value::List(vec![old_edge, reference]));
    heap.release_temporaries_since(0);
    assert!(heap.retained_paths_keep_owners([root].into_iter(), true));

    let Value::List(children) = heap.get_mut(root) else {
        unreachable!()
    };
    children.remove(0);
    // Keep the endpoint alive across actual collection, without keeping its old parent.
    assert_eq!(
        heap.collect([root, owner].into_iter()),
        Some(vec![(callback, reference)])
    );
    assert!(heap.slots[old_edge as usize].value.is_none());
    let new_edge = (0..=heap.free_count)
        .map(|_| heap.alloc(Value::List(Vec::new())))
        .find(|&slot| slot == old_edge)
        .expect("the free slot must actually be reused");
    assert_eq!(
        old_edge, new_edge,
        "the recorded slot must have been reused"
    );
    let Value::List(children) = heap.get_mut(root) else {
        unreachable!()
    };
    children.insert(0, new_edge);
    heap.release_temporaries_since(0);
    assert!(!heap.retained_paths_keep_owners([root].into_iter(), true));
}

#[test]
fn an_unavailable_native_edge_cannot_keep_a_finalizer_alive() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let owner = ctx.alloc(Value::List(Vec::new()));
    ctx.heap.register_finalizer(owner);
    let native = stdlib::call(&mut ctx, "_bz2.BZ2Decompressor", &[], &[]).unwrap();
    let Value::Bz2Decompressor(state) = ctx.heap.get(native) else {
        unreachable!()
    };
    let state = state.clone();
    state.borrow_mut().unused_data = owner;
    ctx.heap.release_temporaries_since(0);
    assert!(
        ctx.heap
            .retained_paths_keep_owners([native].into_iter(), true)
    );
    let mut busy = state.borrow_mut();
    let unavailable = ctx
        .heap
        .retained_paths_keep_owners([native].into_iter(), true);
    assert!(
        !unavailable,
        "an inaccessible only edge must not authorize the skip"
    );
    busy.unused_data = NONE;
    drop(busy);
    assert!(
        !ctx.heap
            .retained_paths_keep_owners([native].into_iter(), true)
    );
}
