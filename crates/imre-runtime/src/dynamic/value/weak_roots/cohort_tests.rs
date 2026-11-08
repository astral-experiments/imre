use super::super::{FrameOwner, NONE, Value};
use crate::dynamic::{ffi, stdlib, weakref};

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
            true,
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
            true,
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

#[test]
fn iterator_paths_recheck_the_suspended_operands() {
    use super::RootPaths;

    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let scope = ctx.scope("suspended", None);
    let heap = &mut ctx.heap;
    let target = heap.alloc(Value::List(Vec::new()));
    let cursor = heap.alloc(Value::LoopIterator {
        source: NONE,
        peeked: Some(target),
    });
    let generator = heap.alloc(Value::Generator {
        owner: FrameOwner::Module(NONE),
        frame: None,
        name: NONE,
        qualname: NONE,
        lineno: 0,
        code: NONE,
        scope,
        iterators: vec![cursor],
        protected: Vec::new(),
        offset: Some(0),
        running: false,
    });
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

    let Value::Generator { iterators, .. } = heap.get_mut(generator) else {
        unreachable!()
    };
    iterators.clear();
    assert!(!paths.valid(heap, globals));
}

#[test]
fn suspended_cleanup_keeps_exit_methods_and_a_pending_return() {
    let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
    let namespace = ctx.dict(Vec::new());
    let source = ctx.text(
        r#"
class Guard:
    def __enter__(self): pass
    def __exit__(self, *exc): pass

def hold():
    with Guard(), Guard():
        try:
            return object()
        finally:
            yield

g = hold()
next(g)
"#,
    );
    ctx.exec_source(source, namespace, None, None).unwrap();
    let generator = ctx.dict_named_item(namespace, "g").unwrap();
    let Value::Generator { protected, .. } = ctx.heap.get(generator) else {
        unreachable!()
    };
    let targets: Vec<_> = protected.iter().flat_map(|block| block.roots()).collect();
    assert_eq!(targets.len(), 3);
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

    let file = stdlib::BinaryFile::open(
        concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
        NONE,
        "rb",
        None,
    )
    .unwrap()
    .into_value()
    .unwrap();
    let descriptor = heap.alloc_descriptor_owner(file).unwrap();
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
