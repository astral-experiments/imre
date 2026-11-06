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
    heap.finalizer_operands.push(std::cell::Cell::new(u16::MAX));
    assert!(heap.operand_keeps_finalizer(owner, 1, 0));
    assert_ne!(heap.finalizer_operands[0].get(), u16::MAX);
    assert!(!heap.operand_keeps_finalizer(owner, 0, 0));

    heap.enter(scope, NONE, FrameOwner::Module(NONE));
    assert!(heap.operand_keeps_finalizer(owner, 0, 0));
    let Value::LoopIterator { source, .. } = heap.get_mut(iterator) else {
        unreachable!()
    };
    *source = other;
    assert!(!heap.operand_keeps_finalizer(owner, 0, 0));
    assert!(heap.operand_keeps_finalizer(other, 0, 0));

    heap.leave(NONE, false);
    assert!(!heap.operand_keeps_finalizer(other, 0, 0));
    heap.leave(NONE, false);
    assert!(!heap.operand_keeps_finalizer(other, usize::MAX, 0));
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
    let stdin = stdlib::io::rooted_input_chain(&ctx);
    let heap = &mut ctx.heap;
    let Value::Scope(bindings) = heap.get_mut(scope) else {
        unreachable!()
    };
    bindings.names.insert("held".into(), held);
    bindings.names.insert("refs".into(), refs);
    heap.release_temporaries_since(0);

    heap.learn_finalizer_paths(globals, stdin.as_ref());
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), None));
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));

    for (list, before_discard) in [(refs, Some(0)), (held, None)] {
        let Value::List(items) = heap.get_mut(list) else {
            unreachable!()
        };
        let old = std::mem::replace(&mut items[0], NONE);
        assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), before_discard));
        let Value::List(items) = heap.get_mut(list) else {
            unreachable!()
        };
        items[0] = old;
    }
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));

    let extra = ctx.instance(class, None);
    let heap = &mut ctx.heap;
    heap.register_finalizer(extra);
    heap.release_temporaries_since(0);
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), None));
    heap.keep(extra);
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), None));
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));
    heap.release_temporaries_since(0);
    heap.take_finalizer(extra, false);
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));

    heap.leave(NONE, false);
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), None));
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));
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
    let stdin = stdlib::io::rooted_input_chain(&ctx);
    let heap = &mut ctx.heap;
    heap.register_finalizer(source);
    heap.register_finalizer(item);
    heap.release_temporaries_since(0);
    heap.keep(cursor);
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(1)));
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), None));
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));

    for _ in 0..32 {
        heap.enter(scope, NONE, FrameOwner::Module(NONE));
    }
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));
    let Value::LoopIterator { peeked, .. } = heap.get_mut(cursor) else {
        unreachable!();
    };
    *peeked = None;
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), Some(0)));
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

#[test]
fn iterator_paths_recheck_the_suspended_operands() {
    use super::{Heap, RootPaths};

    let mut heap = Heap::new();
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
        scope: NONE,
        iterators: vec![cursor],
        protected: Vec::new(),
        offset: Some(0),
        running: false,
    });
    let mapped = heap.alloc(Value::Map {
        function: NONE,
        inputs: [generator].into(),
        strict: false,
    });
    let zipped = heap.alloc(Value::Zip {
        inputs: [mapped].into(),
        strict: false,
    });
    let globals = [zipped, NONE];
    let paths = RootPaths::discover(&heap, globals, [target].into_iter(), false, false).unwrap();
    assert!(paths.nodes.contains(&target));
    assert!(paths.valid(&heap, globals));

    let Value::LoopIterator { peeked, .. } = heap.get_mut(cursor) else {
        unreachable!()
    };
    *peeked = None;
    assert!(!paths.valid(&heap, globals));
    let Value::LoopIterator { peeked, .. } = heap.get_mut(cursor) else {
        unreachable!()
    };
    *peeked = Some(target);
    assert!(paths.valid(&heap, globals));

    let Value::Generator { iterators, .. } = heap.get_mut(generator) else {
        unreachable!()
    };
    iterators.clear();
    assert!(!paths.valid(&heap, globals));
}
