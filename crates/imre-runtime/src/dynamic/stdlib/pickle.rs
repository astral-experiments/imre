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
                        encoding::tuple_end(&mut self.out, self.protocol, items.len());
                        if !items.is_empty() {
                            self.remember(ctx, obj)?;
                        }
                    }
                    Value::Function { .. } | Value::Class { .. } | Value::Native(_) => {
                        self.save_global(ctx, obj, None)?;
                    }
                    _ => self.reduce(ctx, obj)?,
                }
                Ok(())
            },
        )
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
        encoding::global(
            &mut self.out,
            self.protocol,
            &module,
            &name,
            self.fix_imports,
        )
        .map_err(|e| {
            if e.kind == "PicklingError" {
                pickling_error(ctx, e.message)
            } else {
                e
            }
        })?;
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
                    return Err(Error::new(
                        "NotImplementedError",
                        format!("native pickle has no reducer for {}", ctx.type_name(obj)),
                    ));
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
        if fields[2..].iter().any(|&field| field != NONE) {
            return Err(Error::new(
                "NotImplementedError",
                "native pickle does not yet support reduction state, items or state setters",
            ));
        }
        self.save(ctx, fields[0])?;
        self.save(ctx, fields[1])?;
        self.out.push(b'R');
        self.remember(ctx, obj)
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
