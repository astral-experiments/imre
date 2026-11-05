use super::super::{FrameOwner, NONE, Value};
use crate::dynamic::{ffi, stdlib, weakref};

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
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), false));
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), true));

    for (list, before_discard) in [(refs, true), (held, false)] {
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
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), true));

    let extra = ctx.instance(class, None);
    let heap = &mut ctx.heap;
    heap.register_finalizer(extra);
    heap.release_temporaries_since(0);
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), false));
    heap.keep(extra);
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), false));
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), true));
    heap.release_temporaries_since(0);
    heap.take_finalizer(extra, false);
    assert!(heap.cached_finalizers_kept(globals, stdin.as_ref(), true));

    heap.leave(NONE, false);
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), false));
    assert!(!heap.cached_finalizers_kept(globals, stdin.as_ref(), true));
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
    assert!(paths.nodes.contains(&first));
    assert!(!paths.nodes.contains(&second));
    assert!(paths.valid(&heap, globals));
    let Value::List(items) = heap.get_mut(root) else {
        unreachable!()
    };
    items.remove(0);
    assert!(!paths.valid(&heap, globals));
}
