use super::{THREAD, Value};
use crate::dynamic::ffi;
use std::sync::Arc;

#[test]
fn first_access_retries_inherited_initialization_in_a_fresh_dictionary() {
    let foreign_owner = THREAD.with(Arc::downgrade);
    std::thread::spawn(move || {
        let mut ctx = ffi::new_context(
            serde_json::json!({"paths": []}),
            std::iter::empty::<String>(),
        );
        let globals = ctx.dict(Vec::new());
        ctx.heap.keep(globals);
        let source = ctx.text(
            r#"
from _thread import _local

seen = []
pos = []
kw = {}
class Base(_local):
    def __init__(self, value, *, option):
        assert value is pos and option is kw
        assert self.__dict__ == {}
        seen.append(self.__dict__)
        self.attempt = len(seen)
        if self.attempt == 2:
            raise ValueError('retry')

class Local(Base):
    pass

local = Local(pos, option=kw)
creator = local.__dict__
assert seen == [{'attempt': 1}]
"#,
        );
        ctx.exec_source(source, globals, None, None).unwrap();
        let object = ctx.dict_named_item(globals, "local").unwrap();
        let storage = ctx.scalar_value(object);
        let Value::ThreadLocal(local) = ctx.heap.get_mut(storage) else {
            unreachable!()
        };
        // Keep the heap on this worker. Only the existing dictionary's live thread owner
        // changes, giving the worker an ordinary first access to the constructed local.
        local.threads[0].0 = foreign_owner;
        let error = ctx.lookup_attr(object, "__dict__").unwrap_err();
        assert!(ctx.is_builtin_error(&error, "ValueError"), "{error:?}");
        assert_eq!(error.message, "retry");

        let source = ctx.text(
            r#"
local.after_retry = True
assert local.attempt == 3
assert len(seen) == 3
assert creator is seen[0] and creator == {'attempt': 1}
assert seen[1] == {'attempt': 2}
assert local.__dict__ is seen[2]
assert seen[2] == {'attempt': 3, 'after_retry': True}
"#,
        );
        ctx.exec_source(source, globals, None, None).unwrap();
    })
    .join()
    .unwrap();
}
