use super::super::value::DictViewKind;
use super::{Context, Error, Id, NONE, Result, Value};
use std::collections::HashMap;
mod encoding;

pub(in super::super) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
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

/// Produce the protocol 2+ default reduction for an ordinary Python instance with no new arguments.
fn default_instance_reduce(ctx: &mut Context, receiver: Id, protocol: i32) -> Result<Id> {
    let Value::Instance { class, .. } = *ctx.heap.get(receiver) else {
        return Err(unsupported_reducer(ctx, receiver));
    };
    // A native base may have state beyond the Python dictionary and slots.
    let Value::Class { ancestors, .. } = ctx.heap.get(class) else {
        return Err(unsupported_reducer(ctx, receiver));
    };
    if protocol < 2
        || ancestors
            .iter()
            .any(|&base| matches!(ctx.heap.get(base), Value::Native(kind) if kind != "object"))
        || ctx.class_attr(class, "__getnewargs_ex__").is_some()
        || ctx.class_attr(class, "__getnewargs__").is_some()
    {
        return Err(unsupported_reducer(ctx, receiver));
    }

    let copyreg = ctx.module("copyreg")?;
    let newobj = ctx.lookup_attr(copyreg, "__newobj__")?;
    ctx.heap.keep(newobj);
    let class = ctx.exact_type(receiver)?;
    let args = ctx.alloc(Value::Tuple(vec![class]));
    let getstate = ctx.lookup_attr(receiver, "__getstate__")?;
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
                    Value::Function { .. } | Value::Class { .. } | Value::Native(_) => {
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
        self.save_reduce(ctx, obj, func, args, NONE, NONE)
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
        let module = ctx.lookup_attr(obj, "__module__")?;
        let module = ctx.str_value(module)?.to_owned();
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
        let reduction = match ctx.lookup_attr(obj, "__reduce_ex__") {
            Ok(hook) => {
                let protocol = ctx.int(i64::from(self.protocol));
                ctx.call(hook, vec![protocol], Vec::new())?
            }
            Err(e) if ctx.is_attribute_error(&e) => match ctx.lookup_attr(obj, "__reduce__") {
                Ok(hook) => ctx.call(hook, Vec::new(), Vec::new())?,
                Err(e) if ctx.is_attribute_error(&e) => {
                    return Err(unsupported_reducer(ctx, obj));
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
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
        let mapping_items = optional(4);
        if mapping_items != NONE
            && !ctx.is_native_iterator(mapping_items)
            && ctx.slot(mapping_items, "__next__").is_none()
        {
            return Err(pickling_error(
                ctx,
                format!(
                    "fifth item of the tuple returned by __reduce__ must be an iterator, not {}",
                    ctx.type_name(mapping_items)
                ),
            ));
        }
        if optional(3) != NONE || optional(5) != NONE {
            return Err(Error::new(
                "NotImplementedError",
                "native pickle does not yet support reduction list items or state setters",
            ));
        }
        self.save_reduce(ctx, obj, fields[0], fields[1], optional(2), mapping_items)
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
        mapping_items: Id,
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
