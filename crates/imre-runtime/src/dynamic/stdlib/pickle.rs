use super::super::value::{DictViewKind, ELLIPSIS, NOT_IMPLEMENTED};
use super::{Context, Error, Id, NONE, Result, Value};
use std::collections::HashMap;
mod encoding;

pub(in super::super) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
    // Keep the dictionary itself so that live registrations are visible, even if
    // the copyreg module attribute is later rebound.
    let copyreg = ctx.module("copyreg")?;
    let table = ctx.lookup_attr(copyreg, "dispatch_table")?;
    if !matches!(ctx.heap.get(table), Value::Dict(_)) {
        return Err(Error::new(
            "RuntimeError",
            format!(
                "copyreg.dispatch_table should be a dict, not {}",
                ctx.type_name(table)
            ),
        ));
    }
    ctx.builtins.insert("_pickle.dispatch_table".into(), table);
    let version = ctx.int(5);
    ctx.bind(scope, "HIGHEST_PROTOCOL", version);
    ctx.bind(scope, "DEFAULT_PROTOCOL", version);
    let mut base = ctx.builtin("Exception");
    for name in ["PickleError", "PicklingError"] {
        let fields = ctx.scope("_pickle", Some(scope));
        let module_name = ctx.text("_pickle");
        ctx.bind(fields, "__module__", module_name);
        let qualname = ctx.text(name);
        ctx.bind(fields, "__qualname__", qualname);
        let class = ctx.allocate_class(name.into(), fields, vec![base])?;
        ctx.bind(scope, name, class);
        ctx.builtins.insert(format!("_pickle.{name}"), class);
        base = class;
    }
    Ok(())
}

fn pickling_error(ctx: &Context, message: impl Into<String>) -> Error {
    let mut error = Error::new("PicklingError", message);
    error.class = ctx.builtins["_pickle.PicklingError"];
    error
}

fn unsupported_reducer(ctx: &Context, obj: Id) -> Error {
    Error::new(
        "NotImplementedError",
        format!("native pickle has no reducer for {}", ctx.type_name(obj)),
    )
}

/// Preserve the native base for legacy reductions and consult constructor hooks for newer protocols.
fn native_layout_reduce(
    ctx: &mut Context,
    receiver: Id,
    class: Id,
    protocol: i32,
    cursor: Option<(Id, &'static str)>,
) -> Result<Id> {
    let copyreg = ctx.module("copyreg")?;
    if protocol < 2 {
        let reducer = ctx.lookup_attr(copyreg, "_reduce_ex_with_base")?;
        ctx.heap.keep(reducer);
        let reported_class = ctx.lookup_attr(receiver, "__class__")?;
        ctx.heap.keep(reported_class);
        let protocol = ctx.int(i64::from(protocol));
        let base = if ctx.namedtuple_fields(reported_class).is_some() {
            ctx.builtin("tuple")
        } else {
            ctx.allocation_base(reported_class)
                .filter(|&base| ctx.itertools_layout(base).is_some())
                .unwrap_or(NONE)
        };
        return ctx.call(
            reducer,
            vec![receiver, protocol, reported_class, base],
            Vec::new(),
        );
    }
    let extended = ctx.class_attr(class, "__getnewargs_ex__");
    let hook = extended.or_else(|| ctx.class_attr(class, "__getnewargs__"));
    let Some(hook) = hook else {
        return reduce_without_newargs(ctx, receiver, class, cursor);
    };
    let method = ctx.bind_attribute(hook, receiver, class)?;
    let args = ctx.call(method, Vec::new(), Vec::new())?;
    ctx.heap.keep(args);
    let reducer = ctx.lookup_attr(copyreg, "_reduce_newobj")?;
    let extended = super::super::builtins::boolean(extended.is_some());
    ctx.call(reducer, vec![receiver, args, extended], Vec::new())
}

/// Reduce an ordinary Python instance using the state rules for its pickle protocol.
fn default_instance_reduce(ctx: &mut Context, receiver: Id, protocol: i32) -> Result<Id> {
    let Value::Instance { class, .. } = *ctx.heap.get(receiver) else {
        return Err(unsupported_reducer(ctx, receiver));
    };
    let cursor = ctx
        .allocation_base(class)
        .and_then(|base| ctx.itertools_layout(base).map(|name| (base, name)));
    if cursor.is_some() || ctx.namedtuple_fields(class).is_some() {
        return native_layout_reduce(ctx, receiver, class, protocol, cursor);
    }
    // A native base may have state beyond the Python dictionary and slots.
    let Value::Class { ancestors, .. } = ctx.heap.get(class) else {
        return Err(unsupported_reducer(ctx, receiver));
    };
    if ancestors
        .iter()
        .any(|&base| matches!(ctx.heap.get(base), Value::Native(kind) if kind != "object"))
    {
        return Err(unsupported_reducer(ctx, receiver));
    }
    if protocol < 2 {
        if !ctx.uses_object_new(ctx.allocation_base(class)) {
            return Err(unsupported_reducer(ctx, receiver));
        }
        let copyreg = ctx.module("copyreg")?;
        let reducer = ctx.lookup_attr(copyreg, "_reduce_ex")?;
        ctx.heap.keep(reducer);
        let protocol = ctx.int(i64::from(protocol));
        return ctx.call(reducer, vec![receiver, protocol], Vec::new());
    }
    if ctx.class_attr(class, "__getnewargs_ex__").is_some()
        || ctx.class_attr(class, "__getnewargs__").is_some()
    {
        return Err(unsupported_reducer(ctx, receiver));
    }

    reduce_without_newargs(ctx, receiver, class, None)
}

fn reduce_without_newargs(
    ctx: &mut Context,
    receiver: Id,
    class: Id,
    cursor: Option<(Id, &'static str)>,
) -> Result<Id> {
    let copyreg = ctx.module("copyreg")?;
    let newobj = ctx.lookup_attr(copyreg, "__newobj__")?;
    ctx.heap.keep(newobj);
    let args = ctx.alloc(Value::Tuple(vec![class]));
    let getstate = ctx.lookup_attr(receiver, "__getstate__")?;
    if let Some((base, name)) = cursor
        && matches!(*ctx.heap.get(getstate), Value::Method { callable, receiver: bound, .. }
            if bound == receiver && ctx.builtins.get("object.__getstate__") == Some(&callable))
    {
        // The default Python state cannot reconstruct the saved iterator and callable.
        let name = if class == base {
            name
        } else {
            ctx.type_name(receiver)
        };
        return Err(Error::new(
            "TypeError",
            format!("cannot pickle '{name}' object"),
        ));
    }
    let state = ctx.call(getstate, Vec::new(), Vec::new())?;
    Ok(ctx.alloc(Value::Tuple(vec![newobj, args, state, NONE, NONE])))
}

fn default_reduction(ctx: &mut Context, obj: Id, protocol: i32) -> Result<Id> {
    if !matches!(ctx.heap.get(obj), Value::DirEntry(_)) {
        return default_instance_reduce(ctx, obj, protocol);
    }
    if protocol >= 2 {
        return Err(Error::new(
            "TypeError",
            format!("cannot pickle '{}' object", ctx.type_name(obj)),
        ));
    }
    // copyreg's legacy reducer records the class even when its instances cannot be created.
    let copyreg = ctx.module("copyreg")?;
    let reconstructor = ctx.lookup_attr(copyreg, "_reconstructor")?;
    let class = ctx.exact_type(obj)?;
    let base = ctx.builtin("object");
    let args = ctx.alloc(Value::Tuple(vec![class, base, NONE]));
    Ok(ctx.alloc(Value::Tuple(vec![reconstructor, args])))
}

/// Delegate to the instance hook only when its class overrides object's reducer.
pub(in crate::dynamic) fn object_reduce(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method object.{name}() needs an argument"),
        ));
    };
    let class = if matches!(ctx.heap.get(receiver), Value::DirEntry(_)) {
        "DirEntry"
    } else {
        ctx.type_name(receiver)
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{class}.{name}() takes no keyword arguments"),
        ));
    }
    if name == "__reduce__" {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{class}.{name}() takes no arguments ({} given)", rest.len()),
            ));
        }
        return default_reduction(ctx, receiver, 0);
    }
    let [protocol] = rest else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{class}.{name}() takes exactly one argument ({} given)",
                rest.len()
            ),
        ));
    };
    let protocol = ctx
        .index_integer(*protocol)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    let reducer = match ctx.lookup_attr(receiver, "__reduce__") {
        Ok(reducer) => reducer,
        Err(error) if ctx.is_attribute_error(&error) => {
            return default_reduction(ctx, receiver, protocol);
        }
        Err(error) => return Err(error),
    };
    let class = ctx.exact_type(receiver)?;
    if ctx.lookup_attr(class, "__reduce__")? == ctx.builtin("object.__reduce__") {
        return default_reduction(ctx, receiver, protocol);
    }
    ctx.call(reducer, Vec::new(), Vec::new())
}

const BATCH_SIZE: usize = 1000;

struct Pickler {
    protocol: u8,
    fix_imports: bool,
    memo: HashMap<Id, u32>,
    out: Vec<u8>,
}

impl Pickler {
    fn remember(&mut self, ctx: &mut Context, obj: Id) -> Result<()> {
        // Reentrant reduction can finish the same object before its outer REDUCE.
        if self.memo.contains_key(&obj) {
            return Err(Error::new(
                "NotImplementedError",
                "native pickle does not yet support recursive reductions",
            ));
        }
        let index = u32::try_from(self.memo.len())
            .map_err(|_| Error::new("OverflowError", "pickle memo too large"))?;
        self.memo
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        self.memo.insert(obj, index);
        ctx.heap.keep(obj);
        encoding::memo_put(&mut self.out, self.protocol, index);
        Ok(())
    }

    fn save(&mut self, ctx: &mut Context, obj: Id) -> Result<()> {
        if let Some(&index) = self.memo.get(&obj) {
            encoding::memo_get(&mut self.out, self.protocol, index);
            return Ok(());
        }
        ctx.with_slot_depth(
            "maximum recursion depth exceeded while pickling an object",
            |ctx| {
                match ctx.heap.get(obj) {
                    Value::None => self.out.push(b'N'),
                    Value::Bool(value) => encoding::boolean(&mut self.out, self.protocol, *value),
                    Value::Int(value) => encoding::int(&mut self.out, self.protocol, value)?,
                    Value::Float(value) => encoding::float(&mut self.out, self.protocol, *value),
                    Value::Str(text) => {
                        encoding::string(&mut self.out, self.protocol, text)?;
                        self.remember(ctx, obj)?;
                    }
                    Value::Tuple(items) => {
                        let items = items.clone();
                        encoding::tuple_start(&mut self.out, self.protocol, items.len());
                        for &item in &items {
                            self.save(ctx, item)?;
                        }
                        if let Some(&index) = self.memo.get(&obj) {
                            // A mutable child finished this same tuple while its elements were saved.
                            encoding::recursive_tuple(
                                &mut self.out,
                                self.protocol,
                                items.len(),
                                index,
                            );
                        } else {
                            encoding::tuple_end(&mut self.out, self.protocol, items.len());
                            if !items.is_empty() {
                                self.remember(ctx, obj)?;
                            }
                        }
                    }
                    Value::Bytes(bytes) if self.protocol >= 3 => {
                        encoding::bytes(&mut self.out, self.protocol, bytes)?;
                        self.remember(ctx, obj)?;
                    }
                    Value::Bytes(_) => self.save_legacy_bytes(ctx, obj)?,
                    Value::List(items) => {
                        let length = items.len();
                        self.save_list(ctx, obj, length)?;
                    }
                    Value::Dict(_) => self.save_dict(ctx, obj)?,
                    Value::Function { .. } => {
                        self.save_global(ctx, obj, None)?;
                    }
                    _ => self.reduce(ctx, obj)?,
                }
                Ok(())
            },
        )
    }

    /// Protocols before BINBYTES reconstruct through bytes() or _codecs.encode(text, "latin1").
    fn save_legacy_bytes(&mut self, ctx: &mut Context, obj: Id) -> Result<()> {
        let Value::Bytes(bytes) = ctx.heap.get(obj) else {
            unreachable!()
        };
        let (func, args) = if bytes.is_empty() {
            (ctx.builtin("bytes"), Vec::new())
        } else {
            let text = crate::objects::Codec::Latin1.decode(bytes, None)?;
            let text = ctx.text(text);
            let latin1 = ctx
                .heap
                .source_string_constant("latin1")
                .ok_or_else(crate::Error::allocation)?;
            (ctx.builtin("codecs.encode"), vec![text, latin1])
        };
        let args = ctx.alloc(Value::Tuple(args));
        self.save_reduce(ctx, obj, func, args, NONE, [NONE; 2])
    }

    fn save_list(&mut self, ctx: &mut Context, obj: Id, length: usize) -> Result<()> {
        self.out
            .extend_from_slice(if self.protocol == 0 { b"(l" } else { b"]" });
        self.remember(ctx, obj)?;
        let iterator = ctx.iterator(obj)?;
        let batched = self.protocol > 0 && length > 1;
        let mut count = 0;
        while let Some(item) = ctx.next_item(iterator)? {
            if batched && count == 0 {
                self.out.push(b'('); // MARK
            }
            // A reducer may remove its own list entry or replace a later one.
            ctx.heap.keep(item);
            self.save(ctx, item)?;
            if batched {
                count += 1;
                if count == BATCH_SIZE {
                    self.out.push(b'e'); // APPENDS
                    count = 0;
                }
            } else {
                self.out.push(b'a'); // APPEND
                if self.protocol > 0 {
                    // The binary singleton fast path writes only the original item.
                    break;
                }
            }
        }
        if count > 0 {
            self.out.push(b'e');
        }
        Ok(())
    }

    fn save_dict(&mut self, ctx: &mut Context, obj: Id) -> Result<()> {
        if self.protocol == 0 {
            self.out.extend_from_slice(b"(d"); // MARK, DICT
        } else {
            self.out.push(b'}'); // EMPTY_DICT
        }
        self.remember(ctx, obj)?;

        let entries = ctx.stored_entries(obj);
        if entries.is_empty() {
            return Ok(());
        }
        if self.protocol == 0 {
            // The text protocol iterates dict.items(), including its mutation checks.
            let iter = ctx.dict_iterator(obj, DictViewKind::Items, false, false);
            while let Some(item) = ctx.next_item(iter)? {
                let Value::Tuple(pair) = ctx.heap.get(item) else {
                    unreachable!("dict items iterator yields pairs");
                };
                self.save_pair(ctx, pair[0], pair[1])?;
                self.out.push(b's'); // SETITEM
            }
            return Ok(());
        }
        let size = entries.len();
        if size == 1 {
            let (key, value) = entries[0];
            self.save_pair(ctx, key, value)?;
            self.out.push(b's'); // SETITEM
            return Ok(());
        }

        let mut position = 0;
        loop {
            self.out.push(b'('); // MARK
            let mut count = 0;
            while count < BATCH_SIZE {
                let Some((slot, key, value)) = ctx.stored_entries(obj).at(position) else {
                    break;
                };
                position = slot + 1;
                self.save_pair(ctx, key, value)?;
                count += 1;
            }
            self.out.push(b'u'); // SETITEMS
            if ctx.stored_entries(obj).len() != size {
                return Err(Error::new(
                    "RuntimeError",
                    "dictionary changed size during iteration",
                ));
            }
            if count < BATCH_SIZE {
                return Ok(());
            }
        }
    }

    fn save_pair(&mut self, ctx: &mut Context, key: Id, value: Id) -> Result<()> {
        // Either reducer may remove this entry before the other object is saved.
        ctx.heap.keep(key);
        ctx.heap.keep(value);
        self.save(ctx, key)?;
        self.save(ctx, value)
    }

    fn save_global(&mut self, ctx: &mut Context, obj: Id, name: Option<String>) -> Result<()> {
        let name = match name {
            Some(name) => name,
            None => {
                let id = match ctx.lookup_attr(obj, "__qualname__") {
                    Err(e) if ctx.is_attribute_error(&e) => ctx.lookup_attr(obj, "__name__")?,
                    result => result?,
                };
                ctx.str_value(id)?.to_owned()
            }
        };
        let module = if matches!(obj, ELLIPSIS | NOT_IMPLEMENTED) {
            "builtins".to_owned()
        } else {
            let module = ctx.lookup_attr(obj, "__module__")?;
            ctx.str_value(module)?.to_owned()
        };
        if name
            .split('.')
            .any(|part| part == "<locals>" || part.is_empty())
        {
            return Err(pickling_error(
                ctx,
                format!("Can't pickle local object '{name}'"),
            ));
        }
        let resolved = if module == "builtins" && super::super::builtins::is_builtin(&name) {
            ctx.builtin(&name)
        } else {
            let mut current = ctx.module(&module).map_err(|e| {
                if ctx.is_builtin_error(&e, "ModuleNotFoundError") {
                    pickling_error(ctx, format!("Can't import module {module}"))
                } else {
                    e
                }
            })?;
            for part in name.split('.') {
                current = ctx.lookup_attr(current, part).map_err(|e| {
                    if ctx.is_attribute_error(&e) {
                        pickling_error(ctx, format!("Can't find {name} in {module}"))
                    } else {
                        e
                    }
                })?;
            }
            current
        };
        if resolved != obj {
            return Err(pickling_error(
                ctx,
                format!("object differs from {module}.{name}"),
            ));
        }
        // Before STACK_GLOBAL, rebuild dotted names as getattr(getattr(outer, ...), ...).
        let (global_name, suffix) = if self.protocol < 4 {
            name.split_once('.').unwrap_or((&name, ""))
        } else {
            (name.as_str(), "")
        };
        let attributes = suffix.split_terminator('.');
        for _ in attributes.clone() {
            let getattr = ctx.builtin("getattr");
            self.save(ctx, getattr)?;
            encoding::tuple_start(&mut self.out, self.protocol, 2);
        }
        encoding::global(
            &mut self.out,
            self.protocol,
            &module,
            global_name,
            self.fix_imports,
        )
        .map_err(|e| {
            if e.kind == "PicklingError" {
                pickling_error(ctx, e.message)
            } else {
                e
            }
        })?;
        for attribute in attributes {
            let attribute = ctx.text(attribute);
            self.save(ctx, attribute)?;
            encoding::tuple_end(&mut self.out, self.protocol, 2);
            self.out.push(b'R'); // REDUCE
        }
        self.remember(ctx, obj)
    }

    fn reduce(&mut self, ctx: &mut Context, obj: Id) -> Result<()> {
        // Some native function handles lack an exposed type. Preserve their
        // global-name lookup until they acquire a Python type.
        if matches!(ctx.heap.get(obj), Value::Native(_)) && ctx.builtin_type_name(obj).is_none() {
            return self.save_global(ctx, obj, None);
        }
        let class = ctx.exact_type(obj)?;
        if class == ctx.builtin("type") {
            let singleton = match ctx.heap.get(obj) {
                Value::Native(name) if name == "NoneType" => NONE,
                Value::Native(name) if name == "ellipsis" => ELLIPSIS,
                Value::Native(name) if name == "NotImplementedType" => NOT_IMPLEMENTED,
                _ => return self.save_global(ctx, obj, None),
            };
            let args = ctx.alloc(Value::Tuple(vec![singleton]));
            return self.save_reduce(ctx, obj, class, args, NONE, [NONE; 2]);
        }
        // The accelerated pickler owns these exact types even when this runtime
        // cannot yet encode their payloads. A registration cannot replace them.
        let reducer = match ctx.heap.get(obj) {
            Value::Set { .. } | Value::ByteArray(_) | Value::SurrogateStr(_) => None,
            _ => {
                let table = ctx.builtins["_pickle.dispatch_table"];
                let (_, index) = ctx.find_dictionary_key(table, class, None)?;
                index.map(|index| ctx.stored_entries(table)[index].1)
            }
        };
        let reduction = if let Some(reducer) = reducer {
            ctx.call(reducer, vec![obj], Vec::new())?
        } else {
            match ctx.heap.get(obj) {
                Value::Class { .. } | Value::Native(_) => return self.save_global(ctx, obj, None),
                Value::Ellipsis => return self.save_global(ctx, obj, Some("Ellipsis".into())),
                Value::NotImplemented => {
                    return self.save_global(ctx, obj, Some("NotImplemented".into()));
                }
                _ => Self::call_reduce_hook(ctx, obj, self.protocol)?,
            }
        };
        ctx.heap.keep(reduction);
        let fields = match ctx.heap.get(reduction) {
            Value::Str(name) => {
                let name = name.clone();
                return self.save_global(ctx, obj, Some(name));
            }
            Value::Tuple(fields) if (2..=6).contains(&fields.len()) => fields.clone(),
            _ => {
                return Err(pickling_error(
                    ctx,
                    "__reduce__ must return a string or a tuple of length 2 through 6",
                ));
            }
        };
        if !ctx.callable(fields[0]) {
            return Err(pickling_error(
                ctx,
                "first item of the tuple returned by __reduce__ must be callable",
            ));
        }
        if !matches!(ctx.heap.get(fields[1]), Value::Tuple(_)) {
            return Err(pickling_error(
                ctx,
                "second item of the tuple returned by __reduce__ must be a tuple",
            ));
        }
        let optional = |index| fields.get(index).copied().unwrap_or(NONE);
        let items = [optional(3), optional(4)];
        for (iterator, position) in items.into_iter().zip(["fourth", "fifth"]) {
            if iterator != NONE
                && !ctx.is_native_iterator(iterator)
                && ctx.slot(iterator, "__next__").is_none()
            {
                let class = ctx.exact_type(iterator)?;
                let name = super::qualified_type(ctx, class)?;
                let message = format!(
                    "{position} item of the tuple returned by __reduce__ must be an iterator, not {}",
                    ctx.str_value(name)?
                );
                return Err(pickling_error(ctx, message));
            }
        }
        if optional(5) != NONE {
            return Err(Error::new(
                "NotImplementedError",
                "native pickle does not yet support reduction state setters",
            ));
        }
        self.save_reduce(ctx, obj, fields[0], fields[1], optional(2), items)
    }

    fn call_reduce_hook(ctx: &mut Context, obj: Id, protocol: u8) -> Result<Id> {
        match ctx.lookup_attr(obj, "__reduce_ex__") {
            Ok(hook) => {
                let protocol = ctx.int(i64::from(protocol));
                ctx.call(hook, vec![protocol], Vec::new())
            }
            Err(e) if ctx.is_attribute_error(&e) => match ctx.lookup_attr(obj, "__reduce__") {
                Ok(hook) => ctx.call(hook, Vec::new(), Vec::new()),
                Err(e) if ctx.is_attribute_error(&e) => Err(unsupported_reducer(ctx, obj)),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    /// Binary protocols fetch the second item before saving the first to select APPENDS.
    fn save_list_items(&mut self, ctx: &mut Context, iterator: Id) -> Result<()> {
        while let Some(first) = ctx.next_item(iterator)? {
            ctx.heap.keep(first);
            if self.protocol == 0 {
                self.save(ctx, first)?;
                self.out.push(b'a'); // APPEND
                continue;
            }
            let Some(second) = ctx.next_item(iterator)? else {
                self.save(ctx, first)?;
                self.out.push(b'a');
                break;
            };
            ctx.heap.keep(second);
            self.out.push(b'('); // MARK
            self.save(ctx, first)?;
            self.save(ctx, second)?;
            for _ in 2..BATCH_SIZE {
                let Some(item) = ctx.next_item(iterator)? else {
                    self.out.push(b'e'); // APPENDS
                    return Ok(());
                };
                ctx.heap.keep(item);
                self.save(ctx, item)?;
            }
            self.out.push(b'e');
        }
        Ok(())
    }

    /// Binary protocols fetch a second pair before serializing the first to select SETITEMS.
    fn save_mapping_items(&mut self, ctx: &mut Context, iterator: Id) -> Result<()> {
        fn pair(ctx: &Context, item: Id) -> Result<(Id, Id)> {
            match ctx.heap.get(ctx.scalar_value(item)) {
                Value::Tuple(fields) if fields.len() == 2 => Ok((fields[0], fields[1])),
                _ => Err(Error::new(
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                )),
            }
        }

        while let Some(first) = ctx.next_item(iterator)? {
            let (key, value) = pair(ctx, first)?;
            if self.protocol == 0 {
                self.save_pair(ctx, key, value)?;
                self.out.push(b's'); // SETITEM
                continue;
            }
            ctx.heap.keep(first);
            let Some(mut next) = ctx.next_item(iterator)? else {
                self.save_pair(ctx, key, value)?;
                self.out.push(b's');
                break;
            };
            ctx.heap.keep(next);
            self.out.push(b'('); // MARK
            self.save_pair(ctx, key, value)?;
            let mut count = 1;
            loop {
                let (key, value) = pair(ctx, next)?;
                self.save_pair(ctx, key, value)?;
                count += 1;
                if count == BATCH_SIZE {
                    break;
                }
                let Some(item) = ctx.next_item(iterator)? else {
                    self.out.push(b'u'); // SETITEMS
                    return Ok(());
                };
                next = item;
            }
            self.out.push(b'u');
        }
        Ok(())
    }

    /// NEWOBJ applies the original class's allocator directly when a reduction names __newobj__.
    fn save_newobj(&mut self, ctx: &mut Context, obj: Id, args: Id) -> Result<()> {
        let Value::Tuple(items) = ctx.heap.get(args) else {
            unreachable!("validated reduction args")
        };
        let Some((&class, tail)) = items.split_first() else {
            return Err(pickling_error(
                ctx,
                "__newobj__ expected at least 1 argument, got 0",
            ));
        };
        let tail = ctx.alloc(Value::Tuple(tail.to_vec()));
        let type_class = ctx.builtin("type");
        if !ctx.instance_of(class, type_class) {
            return Err(pickling_error(
                ctx,
                format!(
                    "first argument to __newobj__() must be a class, not {}",
                    ctx.type_name(class)
                ),
            ));
        }
        let expected = match ctx.lookup_attr(obj, "__class__") {
            Ok(class) => class,
            Err(error) if ctx.is_attribute_error(&error) => ctx.exact_type(obj)?,
            Err(error) => return Err(error),
        };
        if class != expected {
            let repr = ctx.builtin("repr");
            let expected = ctx.call(repr, vec![expected], Vec::new())?;
            let class = ctx.call(repr, vec![class], Vec::new())?;
            return Err(pickling_error(
                ctx,
                format!(
                    "first argument to __newobj__() must be {}, not {}",
                    ctx.str_value(expected)?,
                    ctx.str_value(class)?
                ),
            ));
        }
        self.save(ctx, class)?;
        self.save(ctx, tail)?;
        self.out.push(0x81); // NEWOBJ
        Ok(())
    }

    fn save_reduce(
        &mut self,
        ctx: &mut Context,
        obj: Id,
        func: Id,
        args: Id,
        state: Id,
        [list_items, mapping_items]: [Id; 2],
    ) -> Result<()> {
        let newobj = if self.protocol >= 2 {
            match ctx.lookup_attr(func, "__name__") {
                Ok(name) => ctx.str_value(name).is_ok_and(|name| name == "__newobj__"),
                Err(error) if ctx.is_attribute_error(&error) => false,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        if newobj {
            self.save_newobj(ctx, obj, args)?;
        } else {
            self.save(ctx, func)?;
            self.save(ctx, args)?;
            self.out.push(b'R');
        }
        if let Some(&index) = self.memo.get(&obj) {
            // A reconstructor argument can reach this object through a mutable container.
            self.out.push(b'0'); // POP
            encoding::memo_get(&mut self.out, self.protocol, index);
            return Ok(());
        }
        self.remember(ctx, obj)?;
        if list_items != NONE {
            self.save_list_items(ctx, list_items)?;
        }
        if mapping_items != NONE {
            self.save_mapping_items(ctx, mapping_items)?;
        }
        if state != NONE {
            self.save(ctx, state)?;
            self.out.push(b'b'); // BUILD
        }
        Ok(())
    }
}

pub(super) fn dumps(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!(
                "dumps() takes at most 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let mut obj = args.first().copied();
    let mut protocol = args.get(1).copied();
    let mut fix_imports = None;
    let mut buffer_callback = NONE;
    for (key, value) in kwargs {
        let (slot, position) = match key.as_str() {
            "obj" => (&mut obj, 1),
            "protocol" => (&mut protocol, 2),
            "fix_imports" => {
                fix_imports = Some(*value);
                continue;
            }
            "buffer_callback" => {
                buffer_callback = *value;
                continue;
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("dumps() got an unexpected keyword argument '{key}'"),
                ));
            }
        };
        if slot.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("argument for dumps() given by name ('{key}') and position ({position})"),
            ));
        }
    }
    let obj = obj.ok_or_else(|| {
        Error::new(
            "TypeError",
            "dumps() missing required argument 'obj' (pos 1)",
        )
    })?;
    let fix_imports = match fix_imports {
        Some(value) => ctx.truth(value)?,
        None => true,
    };
    let protocol = match protocol {
        None | Some(NONE) => 5,
        Some(id) => ctx.index_integer(id)?.to_i64().ok_or_else(|| {
            Error::new("OverflowError", "Python int too large to convert to C long")
        })?,
    };
    if protocol > 5 {
        return Err(Error::new("ValueError", "pickle protocol must be <= 5"));
    }
    if (0..5).contains(&protocol) && buffer_callback != NONE {
        return Err(Error::new(
            "ValueError",
            "buffer_callback needs protocol >= 5",
        ));
    }
    let protocol = if protocol < 0 { 5 } else { protocol as u8 };
    let mut pickler = Pickler {
        protocol,
        fix_imports,
        memo: HashMap::new(),
        out: Vec::new(),
    };
    if protocol >= 2 {
        pickler.out.extend([0x80, protocol]);
    }
    pickler.save(ctx, obj)?;
    pickler.out.push(b'.');
    Ok(ctx.alloc(Value::Bytes(pickler.out)))
}
