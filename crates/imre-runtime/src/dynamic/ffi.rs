//! ABI between LLVM-generated Python functions and Imre-owned objects.

use std::ffi::{CStr, c_char};
use std::slice;

use super::*;

fn flag(v: bool) -> Id {
    if v { TRUE } else { FALSE }
}

impl Context {
    /// Call with packed operands and drop their temporary roots after a successful return.
    fn call_packed(
        &mut self,
        callable: Id,
        arguments: Id,
        keywords: Id,
        mark: Option<usize>,
    ) -> Result<Id> {
        let args = self.items(arguments)?;
        let kwargs = self.keyword_args(keywords)?;
        let result = self.call(callable, args, kwargs)?;
        // Older compiled modules identify the boundary by their fresh argument list.
        if let Some(mark) = mark.or_else(|| self.heap.argument_mark(arguments)) {
            self.finish_call_operands(mark, result);
        }
        Ok(result)
    }

    /// Instantiate exception classes used by raise and its explicit cause.
    fn instantiate_exception_class(&mut self, value: Id) -> Result<Id> {
        let base = self.builtin("BaseException");
        if !self.subclass(value, base) {
            return Ok(value);
        }
        let result = self.call(value, Vec::new(), Vec::new())?;
        if !matches!(self.heap.get(result), Value::Raised(_)) {
            let actual = self.exact_type(result)?;
            return Err(Error::new(
                "TypeError",
                format!(
                    "calling {} should have returned an instance of BaseException, not {}",
                    self.repr(value),
                    self.repr(actual)
                ),
            ));
        }
        Ok(result)
    }

    /// Check the mapping and its length before evaluating the pattern's key expressions.
    fn match_mapping(&mut self, subject: Id, minimum: Id) -> Result<Id> {
        if self.dict_storage(subject).is_none()
            && !matches!(self.heap.get(subject), Value::MappingProxy(_))
        {
            let Some(&mapping) = self.classes.get("collections.abc.Mapping") else {
                return Ok(FALSE);
            };
            let class = self.exact_type(subject)?;
            if !self.subclass(class, mapping) {
                return Ok(FALSE);
            }
        }
        if minimum == NONE {
            return Ok(TRUE);
        }
        let size = builtins::call(self, "len", &[subject], &[])?;
        self.op(">=", size, minimum, NONE)
    }

    /// A missing pattern key fails without invoking a defaultdict factory. Present `None`
    /// values survive in the result tuple, alongside the other keys fetched before matching.
    fn match_mapping_keys(&mut self, subject: Id, keys: Id) -> Result<Id> {
        let Value::List(keys) = self.heap.get(keys) else {
            unreachable!("compiled pattern keys")
        };
        let keys = keys.clone();
        if keys.is_empty() {
            return Ok(self.alloc(Value::Tuple(Vec::new())));
        }
        let get = self.lookup_attr(subject, "get")?;
        self.heap.keep(get);
        let seen = self.op("set", NONE, NONE, NONE)?;
        self.heap.keep(seen);
        let object = self.builtin("object");
        let missing = self.instance(object, None);
        self.heap.keep(missing);
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            if builtins::set_member(self, seen, key, false)? {
                let repr = builtins::call(self, "repr", &[key], &[])?;
                return Err(Error::new(
                    "ValueError",
                    format!(
                        "mapping pattern checks duplicate key ({})",
                        self.str_value(repr)?
                    ),
                ));
            }
            builtins::set_add(self, seen, key)?;
            let value = self.call(get, vec![key, missing], Vec::new())?;
            if value == missing {
                return Ok(NONE);
            }
            self.heap.keep(value);
            values.push(value);
        }
        Ok(self.alloc(Value::Tuple(values)))
    }

    /// Fetch keyword fields in source order before evaluating their nested patterns.
    fn match_class(&mut self, subject: Id, class: Id, names: Id) -> Result<Id> {
        if !self.instance_of(subject, class) {
            return Ok(NONE);
        }
        let Value::List(names) = self.heap.get(names) else {
            unreachable!("compiled class pattern keywords")
        };
        let names = names.clone();
        let mut fields = Vec::with_capacity(names.len());
        for name in names {
            let name = self.str_value(name)?.to_owned();
            match self.lookup_attr(subject, &name) {
                Ok(field) => {
                    self.heap.keep(field);
                    fields.push(field);
                }
                Err(error) if self.is_attribute_error(&error) => return Ok(NONE),
                Err(error) => return Err(error),
            }
        }
        Ok(self.alloc(Value::Tuple(fields)))
    }

    /// Apply a replacement field conversion before evaluating its nested format specification.
    fn format_conversion(&mut self, value: Id, conversion: Id) -> Result<Id> {
        let conversion = self
            .str_value(conversion)?
            .chars()
            .next()
            .unwrap_or_default();
        builtins::format_conversion(self, value, conversion)
    }

    pub(super) fn op(&mut self, op: &str, a: Id, b: Id, c: Id) -> Result<Id> {
        match op {
            "match_class" => self.match_class(a, b, c),
            "match_mapping" => self.match_mapping(a, b),
            "match_mapping_keys" => self.match_mapping_keys(a, b),
            "match_sequence_length"
                if self.builtin_slot_accepts(a, "list")
                    || self.builtin_slot_accepts(a, "tuple") =>
            {
                builtins::call(self, "len", &[a], &[])
            }
            "match_sequence_length" => Ok(NONE),
            "lookup" | "lookup_class_local" => {
                let name = self.str_value(b)?.to_owned();
                if op == "lookup_class_local" {
                    if let Some(value) = self.scope_name(a, &name) {
                        return Ok(value);
                    }
                    return self.lookup(self.root_scope(a), &name);
                }
                self.lookup(a, &name)
            }
            "assign" | "assign_expr" | "assign_target" | "assign_named_target" => {
                let mut scope = a;
                if matches!(op, "assign_expr" | "assign_named_target") {
                    while self.scope_data(scope).comprehension.is_some() {
                        scope = self
                            .scope_data(scope)
                            .parent
                            .expect("enclosing comprehension scope");
                    }
                }
                let name = self.str_value(b)?.to_owned();
                let value = self.enum_assignment(scope, &name, c)?;
                let replaced = self.store_source_name(scope, &name, value)?;
                if matches!(op, "assign_target" | "assign_named_target") {
                    Ok(flag(replaced))
                } else {
                    if replaced {
                        self.heap.release_unbound_writers();
                    }
                    Ok(c)
                }
            }
            "declare_global" => {
                let name = self.str_value(b)?.to_owned();
                let Value::Scope(scope) = self.heap.get_mut(a) else {
                    unreachable!()
                };
                scope.globals.insert(name);
                Ok(NONE)
            }
            "delname" | "clearname" => {
                let name = self.str_value(b)?.to_owned();
                let scope = self.binding_scope(a, &name);
                if !self.remove_binding(scope, &name, b)? && op == "delname" {
                    return Err(self.missing_binding(a, scope, &name));
                }
                Ok(NONE)
            }
            "comprehension_scope" => self.comprehension_scope(a, b),
            "import" => {
                let name = self.str_value(b)?.to_owned();
                self.module(&name)
            }
            "from" => self.import_from(a, b),
            "lookup_enter" => self.context_method(a, "__enter__"),
            "lookup_exit" => self.context_method(a, "__exit__"),
            "attr" | "method" => {
                let name = self.str_value(b)?.to_owned();
                if op == "method" {
                    self.lookup_method(a, &name)
                } else {
                    self.lookup_attr(a, &name)
                }
            }
            "setattr" => {
                let name = self.str_value(b)?.to_owned();
                self.set_attr(a, &name, c)?;
                Ok(c)
            }
            "delattr" => {
                let name = self.str_value(b)?.to_owned();
                self.delete_attr(a, &name)?;
                Ok(NONE)
            }
            "get" => self.get_item(a, b),
            "delitem" => {
                self.delete_item(a, b)?;
                Ok(NONE)
            }
            "put" | "keyword" => {
                if op == "keyword" && self.contains(b, a)? {
                    return Err(Error::new(
                        "TypeError",
                        format!("duplicate keyword {}", self.display(b)),
                    ));
                }
                self.put_item(a, b, c)?;
                Ok(c)
            }
            "list" => Ok(self.alloc(Value::List(Vec::new()))),
            "set" => Ok(self.alloc(Value::Set {
                entries: Default::default(),
                frozen: false,
            })),
            "dict" => Ok(self.dict(Vec::new())),
            "tuple" => {
                let vs = self.items(a)?;
                Ok(self.alloc(Value::Tuple(vs)))
            }
            "append" => {
                match self.heap.get_mut(a) {
                    Value::List(v) => {
                        v.try_reserve(1)
                            .map_err(|_| Error::new("MemoryError", ""))?;
                        v.push(b);
                    }
                    Value::Set { frozen: false, .. } => builtins::set_add(self, a, b)?,
                    _ => return Err(Error::new("TypeError", "expected a list or set")),
                }
                Ok(a)
            }
            "extend" => {
                for item in self.items(b)? {
                    self.op("append", a, item, NONE)?;
                }
                Ok(a)
            }
            "update" | "keywords" => {
                let mapping = self.for_each_mapping_item(
                    b,
                    a,
                    super::calls::MappingOperation::Unpack,
                    |ctx, key, val, hash| {
                        if op == "keywords" {
                            ctx.str_value(key)
                                .map_err(|_| Error::new("TypeError", "keywords must be strings"))?;
                            ctx.op("keyword", a, key, val)?;
                        } else {
                            ctx.set_dictionary_key(a, key, val, hash, true)?;
                        }
                        Ok(())
                    },
                )?;
                if !mapping {
                    return Err(Error::new("TypeError", "expected a mapping"));
                }
                Ok(a)
            }
            "unpack" => {
                let count = usize::try_from(self.as_integer(b)?)
                    .map_err(|_| Error::new("ValueError", "invalid unpack length"))?;
                let star = if c == NONE {
                    None
                } else {
                    let star = usize::try_from(self.as_integer(c)?)
                        .map_err(|_| Error::new("ValueError", "invalid starred target"))?;
                    if star >= count {
                        return Err(Error::new("ValueError", "invalid starred target"));
                    }
                    Some(star)
                };
                let items = self.unpack_items(a, count, star)?;
                Ok(self.alloc(Value::List(items)))
            }
            "unpack_take" => {
                let index = usize::try_from(self.as_integer(b)?).expect("compiled unpack index");
                let Value::List(items) = self.heap.get_mut(a) else {
                    unreachable!("private unpack result")
                };
                Ok(std::mem::replace(&mut items[index], NONE))
            }
            "slice_object" => Ok(self.alloc(Value::Slice([a, b, c]))),
            "call" => self.call_packed(a, b, c, None),
            "class" => self.class(a, b, c),
            "coroutine_function" => {
                let Value::Function { code, .. } = *self.heap.get(a) else {
                    return Err(Error::new("SystemError", "invalid coroutine definition"));
                };
                let Value::Code(code) = self.heap.get_mut(code) else {
                    unreachable!("function code")
                };
                code.coroutine = true;
                Ok(a)
            }
            "annotate" | "annotate_deferred" => {
                if let Some(mapping) = self.scope_name(a, "__annotations__") {
                    if op == "annotate_deferred" {
                        let thunks = if let Some(dict) = self
                            .scope_data(a)
                            .class_namespace
                            .as_ref()
                            .and_then(|c| c.annotation_thunks)
                        {
                            dict
                        } else {
                            let dict = self.dict(Vec::new());
                            let Value::Scope(scope) = self.heap.get_mut(a) else {
                                unreachable!()
                            };
                            scope
                                .class_namespace
                                .get_or_insert_with(Default::default)
                                .annotation_thunks = Some(dict);
                            dict
                        };
                        self.put_item(thunks, b, c)?;
                        self.put_item(mapping, b, NONE)?;
                    } else {
                        self.put_item(mapping, b, c)?;
                    }
                }
                Ok(NONE)
            }
            "truth" => Ok(flag(self.truth(a)?)),
            "generator" => self.generator(a, b, c),
            "yielded" => Ok(self.alloc(Value::Yielded(a))),
            "iter" => self.loop_iterator(a),
            "break_iter" => {
                self.release_loop_iterator(a);
                Ok(NONE)
            }
            "more" | "more_else" => {
                let more = self.iterator_has_item(a)?;
                if !more && op == "more_else" {
                    self.heap.release_unbound_writers();
                }
                Ok(flag(more))
            }
            "next" => self
                .next_item(a)?
                .ok_or_else(|| Error::new("StopIteration", "")),
            "not" => Ok(flag(!self.truth(a)?)),
            "neg" | "pos" if let Value::Float(f) = self.heap.get(a) => {
                Ok(self.alloc(Value::Float(if op == "neg" { -*f } else { *f })))
            }
            "neg" | "pos" if let Value::Complex(z) = self.heap.get(a) => {
                if op == "pos" {
                    Ok(a)
                } else {
                    Ok(self.alloc(Value::Complex(super::complex::Complex {
                        real: -z.real,
                        imag: -z.imag,
                    })))
                }
            }
            "neg" | "pos" | "~" => {
                let (slot, symbol) = match op {
                    "neg" => ("__neg__", "-"),
                    "pos" => ("__pos__", "+"),
                    _ => ("__invert__", "~"),
                };
                if let Some(method) = self.bound_slot(a, slot)? {
                    return self.call(method, vec![], vec![]);
                }
                let v = self.as_int(a).map_err(|_| {
                    let name = self.type_name(a);
                    let name = &name[..name.floor_char_boundary(200)];
                    Error::new(
                        "TypeError",
                        format!("bad operand type for unary {symbol}: '{name}'"),
                    )
                })?;
                let result = match op {
                    "neg" => crate::integer_binary(&crate::Int::from(0), &v, 1)?,
                    "~" => crate::integer_binary(&crate::Int::from(-1), &v, 7)?,
                    _ => v,
                };
                let class = (op == "~").then(|| self.flag_class(a)).flatten();
                self.flag_or_integer(class, result)
            }
            "format_conversion" => self.format_conversion(a, b),
            "format_concat" => self.concat_strings(a, b),
            "format" => {
                let value = if b == NONE {
                    a
                } else {
                    self.format_conversion(a, b)?
                };
                builtins::format_value(self, value, (c != NONE).then_some(c))
            }
            "take_error" => {
                let error = self
                    .pending
                    .take()
                    .ok_or_else(|| Error::new("RuntimeError", "no active exception"))?;
                self.heap.release_unbound_writers();
                Ok(self.raised_instance(error))
            }
            "restore_caught" => {
                let enclosing = self.heap.caught_exception(a);
                self.heap.replace_active(enclosing);
                Ok(NONE)
            }
            "matches" => {
                if let Value::Raised(error) = self.heap.get(a) {
                    Ok(flag(self.raised_error_matches(error, b)))
                } else {
                    Ok(FALSE)
                }
            }
            "type" => {
                if !matches!(self.heap.get(a), Value::Raised(_)) {
                    return Err(Error::new("TypeError", "expected exception"));
                }
                self.exact_type(a)
            }
            "exit_context" | "exit_exception" => {
                let (class, traceback, active) = if b == NONE {
                    (NONE, NONE, self.heap.caught_exception(c))
                } else {
                    (self.exact_type(b)?, self.exception_traceback(b)?, b)
                };
                let prior = self.heap.replace_active(active);
                let result = self.call(a, vec![class, b, traceback], Vec::new());
                // The exceptional exit's truth test still runs inside the handler.
                let result = if b == NONE {
                    result
                } else {
                    result.and_then(|value| self.truth(value).map(flag))
                };
                self.heap.replace_active(prior);
                result
            }
            "raise" | "reraise" => {
                let a = if op == "reraise" && a == NONE {
                    self.heap.active_exception()
                } else {
                    a
                };
                if op == "reraise" && a == NONE {
                    return Err(Error::new("RuntimeError", "No active exception to reraise"));
                }
                let exception = self.instantiate_exception_class(a)?;
                let Value::Raised(e) = self.heap.get(exception) else {
                    return Err(Error::new(
                        "TypeError",
                        "exceptions must derive from BaseException",
                    ));
                };
                let mut error = e.clone();
                let trace = error.tracing();
                trace.instance = Some(exception);
                if op == "raise" {
                    if c == TRUE {
                        let cause = self.instantiate_exception_class(b)?;
                        if cause != NONE && !matches!(self.heap.get(cause), Value::Raised(_)) {
                            return Err(Error::new(
                                "TypeError",
                                "exception causes must derive from BaseException",
                            ));
                        }
                        trace.cause = Some(cause);
                    }
                    if exception != a && !self.instance_of(exception, a) {
                        // A class may allocate a different exception. CPython normalizes
                        // the target by calling the requested class with that instance.
                        let Value::Raised(original) = self.heap.get_mut(exception) else {
                            unreachable!()
                        };
                        *original = error;
                        let normalized = self.call(a, vec![exception], Vec::new())?;
                        let Value::Raised(result) = self.heap.get(normalized) else {
                            return Err(Error::new(
                                "TypeError",
                                format!(
                                    "calling {} should have returned an instance of BaseException, not {}",
                                    self.repr(a),
                                    self.type_name(normalized)
                                ),
                            ));
                        };
                        error = result.clone();
                        error.tracing().instance = Some(normalized);
                    }
                    let active = self.heap.active_exception();
                    if active != NONE
                        && error.trace.as_ref().and_then(|trace| trace.instance) != Some(active)
                    {
                        error.context = active;
                    }
                }
                Err(error)
            }
            "unsupported" => Err(Error::new("NotImplementedError", self.str_value(a)?)),
            _ => match op.strip_prefix("inplace:") {
                Some(op) => self.inplace(op, a, b),
                None => self.binary(op, a, b),
            },
        }
    }
}

// The compiler owns all ABI strings and arrays and uses the declarations in
// dynamic/emit.rs. Every pointer lives through its call; module code lives
// until main returns. The context is accessed on one execution thread.
unsafe fn cstr<'a>(ptr: *const c_char) -> &'a str {
    // SAFETY: the LLVM emitter passes a NUL-terminated UTF-8 constant.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .expect("compiler UTF-8 constant")
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_new(
    argc: i32,
    argv: *const *const c_char,
    config: *const c_char,
) -> *mut Context {
    // SAFETY: native main supplies argc/argv and a static JSON constant.
    let config: serde_json::Value =
        serde_json::from_str(unsafe { cstr(config) }).expect("compiler target metadata");
    crate::integer_text::imre_integer_digits_init(u32::from(
        config["isolated"].as_bool().unwrap_or(false),
    ));
    let executable = config["executable"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_default();
    let mut ctx = Context {
        heap: Heap::new(),
        config,
        executable,
        argv: NONE,
        sys_path: NONE,
        default_encoding: NONE,
        contexts: Vec::new(),
        bytearray_exports: Vec::new(),
        sorting_lists: Vec::new(),
        repr_ancestors: Vec::new(),
        token_missing: NONE,
        native_code_objects: HashMap::new(),
        source_builtins: None,
        zip_directories: HashMap::new(),
        codec_entries: Default::default(),
        modules: HashMap::new(),
        classes: HashMap::new(),
        typing: super::typing::Registry::default(),
        logger_wrappers: HashMap::new(),
        builtins: HashMap::new(),
        sources: HashMap::new(),
        call_depth: 0,
        deque_comparison_repeats: 0,
        pending: None,
        slot_depth: 0,
    };
    // SAFETY: ABI main supplies argc pointers, valid to the end of the call.
    let args = unsafe { slice::from_raw_parts(argv, argc as usize) }
        .iter()
        .map(|&s| {
            // SAFETY: native argv entries are valid NUL-terminated strings.
            ctx.text(unsafe { CStr::from_ptr(s) }.to_string_lossy())
        })
        .collect();
    ctx.argv = ctx.alloc(Value::List(args));
    let paths: Vec<String> =
        serde_json::from_value(ctx.config["paths"].clone()).expect("compiler search paths");
    let paths = paths.into_iter().map(|p| ctx.text(p)).collect();
    ctx.sys_path = ctx.alloc(Value::List(paths));
    ctx.default_encoding = ctx.text(crate::sys::default_encoding().as_str());
    let current = ctx.alloc(Value::Context(Default::default()));
    ctx.contexts.push(current);
    ctx.token_missing = ctx.alloc(Value::TokenMissing);
    Box::into_raw(Box::new(ctx))
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    code: Code,
) {
    // SAFETY: all inputs come from generated module definitions in the same binary.
    let (ctx, name, file) = unsafe { (&mut *ctx, cstr(name), cstr(file)) };
    ctx.sources
        .entry(name.to_owned())
        .or_default()
        .push(SourceModule {
            path: file.into(),
            code,
        });
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_finish(ctx: *mut Context) -> i32 {
    // SAFETY: main transfers its unique allocation from imre_dyn_new exactly once.
    let mut ctx = unsafe { Box::from_raw(ctx) };
    match ctx.module("__main__") {
        Ok(_) => 0,
        Err(e) if e.kind == "SystemExit" => match e.exit_status {
            Some(status) => status.report(),
            None => {
                eprintln!("{}", e.message);
                1
            }
        },
        Err(error) if ctx.is_os_error(&error) && error.args.is_some() => {
            ctx.report_os_error(error);
            1
        }
        Err(mut error) => {
            let current = error
                .trace
                .as_ref()
                .and_then(|trace| trace.instance)
                .and_then(|id| {
                    if let Value::Raised(saved) = ctx.heap.get(id) {
                        Some(saved)
                    } else {
                        None
                    }
                })
                .unwrap_or(&error);
            if current.args.is_some() {
                error.message = ctx.exception_message(current);
            }
            eprintln!("{error}");
            1
        }
    }
}

fn finish(ctx: &mut Context, result: Result<Id>) -> Id {
    finish_at(ctx, result, None)
}

fn finish_at(ctx: &mut Context, result: Result<Id>, span: Option<&[u32; 4]>) -> Id {
    match result {
        Ok(id) => {
            ctx.heap.keep(id);
            id
        }
        Err(mut error) => {
            ctx.record_exception_frame(&mut error, span);
            ctx.pending = Some(error);
            NONE
        }
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_lit(
    ctx: *mut Context,
    kind: i32,
    data: *const u8,
    len: usize,
) -> Id {
    // SAFETY: the emitter supplies a valid context and a constant of exactly len bytes.
    let (ctx, data) = unsafe { (&mut *ctx, slice::from_raw_parts(data, len)) };
    // Each literal roots its value and the returned handle; metadata roots every name.
    let roots = if kind == 5 {
        data.iter()
            .filter(|&&byte| byte == 0)
            .count()
            .saturating_add(2)
    } else {
        2
    };
    if let Err(error) = ctx.heap.reserve_temporaries(roots) {
        return finish(ctx, Err(error));
    }
    let value = match kind {
        0 => std::str::from_utf8(data)
            .ok()
            .and_then(crate::parse_integer_literal)
            .map(crate::Int::new)
            .map(Value::Int)
            .ok_or_else(|| Error::new("ValueError", "invalid integer literal")),
        1 | 5 | 6 => {
            let id = std::str::from_utf8(data)
                .map(|text| {
                    if kind == 1 {
                        return ctx.heap.string_literal(text);
                    }
                    if kind == 6 {
                        return ctx.heap.code_name(text);
                    }
                    for name in text.split('\0') {
                        ctx.heap.code_name(name);
                    }
                    NONE
                })
                .map_err(|e| Error::new("UnicodeError", e.to_string()));
            return finish(ctx, id);
        }
        2 => Ok(Value::Bytes(data.to_vec())),
        3 => data
            .try_into()
            .map(|bytes| Value::Float(f64::from_le_bytes(bytes)))
            .map_err(|_| Error::new("SystemError", "invalid float literal")),
        4 if data.len() == 16 => Ok(Value::Complex(super::complex::Complex {
            real: f64::from_le_bytes(data[..8].try_into().unwrap()),
            imag: f64::from_le_bytes(data[8..].try_into().unwrap()),
        })),
        _ => Err(Error::new("SystemError", "invalid literal kind")),
    };
    let value = value.map(|v| ctx.alloc(v));
    finish(ctx, value)
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_op(
    ctx: *mut Context,
    opcode: *const c_char,
    a: Id,
    b: Id,
    c: Id,
) -> Id {
    // SAFETY: legacy call sites pass the same handles, without an instruction position.
    unsafe { imre_dyn_op_at(ctx, opcode, a, b, c, std::ptr::null()) }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_op_at(
    ctx: *mut Context,
    opcode: *const c_char,
    a: Id,
    b: Id,
    c: Id,
    span: *const [u32; 4],
) -> Id {
    // SAFETY: generated native code owns its context and supplies a static opcode.
    let (ctx, op) = unsafe { (&mut *ctx, cstr(opcode)) };
    if ctx.pending.is_some() && op != "take_error" {
        return NONE;
    }
    let reraise = op == "reraise" && (a != NONE || ctx.heap.active_exception() != NONE);
    let result = ctx.op(op, a, b, c);
    if reraise && let Err(error) = result {
        ctx.pending = Some(error);
        return NONE;
    }
    // SAFETY: the emitter supplies null or a native [4 x i32] module constant.
    finish_at(ctx, result, unsafe { span.as_ref() })
}

/// Invoke a compiled call using the root mark taken before its operands were evaluated.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_call_at(
    ctx: *mut Context,
    callable: Id,
    args: Id,
    kwargs: Id,
    mark: usize,
    span: *const [u32; 4],
) -> Id {
    // SAFETY: the generated caller owns the context and supplies a null or static position.
    let (ctx, span) = unsafe { (&mut *ctx, span.as_ref()) };
    if ctx.pending.is_some() {
        return NONE;
    }
    let result = ctx.call_packed(callable, args, kwargs, Some(mark));
    finish_at(ctx, result, span)
}

/// Preserve the native function ABI used by previously compiled modules.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_def(
    ctx: *mut Context,
    code: Code,
    scope: Id,
    name: Id,
    parameters: Id,
    defaults: Id,
) -> Id {
    // SAFETY: this entry point receives the same context-owned handles and native code.
    unsafe {
        imre_dyn_def_with_metadata(
            ctx, code, scope, name, parameters, defaults, name, NONE, NONE, false,
        )
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_def_with_metadata(
    ctx: *mut Context,
    code: Code,
    scope: Id,
    name: Id,
    parameters: Id,
    defaults: Id,
    qualname: Id,
    doc: Id,
    annotate: Id,
    annotation_scope: bool,
) -> Id {
    // SAFETY: all operands are generated from this context; code has the native function ABI.
    let ctx = unsafe { &mut *ctx };
    let location = ctx.heap.current_location().unwrap_or(value::Location {
        file: "<native>".into(),
        line: 1,
        span: None,
    });
    let code = ctx.prelinked_code(code, scope, name, parameters, (qualname, doc), location);
    ctx.define_function(
        code,
        scope,
        name,
        defaults,
        qualname,
        doc,
        annotate,
        annotation_scope,
    )
}

/// Attach the original generator suite to the code object before applying Python decorators.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_generator_body(
    ctx: *mut Context,
    function: Id,
    source: *const c_char,
    body_line: u32,
) {
    // SAFETY: the generated call provides the live context that owns the function.
    let ctx = unsafe { &mut *ctx };
    let Value::Function { code, .. } = *ctx.heap.get(function) else {
        unreachable!("compiled Python function");
    };
    let Value::Code(code) = ctx.heap.get_mut(code) else {
        unreachable!("compiled code object");
    };
    if matches!(code.body, value::Body::Native(_)) {
        // SAFETY: the UTF-8 constant is embedded in the executable and outlives all code objects.
        let source = unsafe { cstr(source) };
        code.body = value::Body::PendingGenerator { source, body_line };
    }
}

/// Static per-definition record emitted beside its native entry point.
#[repr(C)]
pub(super) struct PrelinkedSite {
    entry: Code,
    filename: *const c_char,
    first_line: u32,
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_def_from_site(
    ctx: *mut Context,
    site: *const PrelinkedSite,
    scope: Id,
    name: Id,
    parameters: Id,
    defaults: Id,
    qualname: Id,
    doc: Id,
    annotate: Id,
    annotation_scope: bool,
) -> Id {
    // SAFETY: the compiler supplies a live context and a distinct static record for this definition.
    let (ctx, definition) = unsafe { (&mut *ctx, &*site) };
    let key = site as usize;
    let code = if let Some(&code) = ctx.native_code_objects.get(&key) {
        code
    } else {
        // SAFETY: the filename is a nul-terminated constant in the native executable.
        let file = unsafe { cstr(definition.filename) };
        let code = ctx.prelinked_code(
            definition.entry,
            scope,
            name,
            parameters,
            (qualname, doc),
            value::Location {
                file: file.into(),
                line: definition.first_line,
                span: None,
            },
        );
        ctx.native_code_objects.insert(key, code);
        code
    };
    ctx.define_function(
        code,
        scope,
        name,
        defaults,
        qualname,
        doc,
        annotate,
        annotation_scope,
    )
}

impl Context {
    fn prelinked_code(
        &mut self,
        entry: Code,
        scope: Id,
        name: Id,
        parameters: Id,
        metadata_pair: (Id, Id),
        location: value::Location,
    ) -> Id {
        let filename = self.text(location.file);
        let (qualname, doc) = metadata_pair;
        let module_definition = self.scope_data(scope).parent.is_none();
        let metadata: serde_json::Value = self
            .str_value(parameters)
            .ok()
            .and_then(|v| serde_json::from_str(v).ok())
            .unwrap_or_default();
        let mut closure_supported = metadata["scope_complete"].as_bool().unwrap_or(false)
            && !metadata["class_cell"].as_bool().unwrap_or(false);
        let mut freevars = Vec::new();
        if closure_supported {
            for name in metadata["scope_reads"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
            {
                let mut cursor = Some(scope);
                while let Some(frame_id) = cursor {
                    let frame = self.scope_data(frame_id);
                    if frame.parent.is_none()
                        || (frame.class_body.is_none() && frame.globals.contains(name))
                    {
                        break;
                    }
                    if frame.class_body.is_none()
                        && (frame.closure.declared.contains(name)
                            || frame.closure.cells.contains_key(name))
                    {
                        freevars.push(name.to_owned());
                        break;
                    }
                    if !frame.closure.declared_complete {
                        closure_supported = false;
                        freevars.clear();
                        break;
                    }
                    cursor = frame.parent;
                }
                if !closure_supported {
                    break;
                }
            }
        }
        self.alloc(Value::Code(value::CodeObject {
            body: value::Body::Native(entry),
            name,
            qualname,
            doc,
            parameters,
            filename,
            first_line: location.line,
            module_definition,
            coroutine: false,
            nested: Vec::new(),
            freevars,
            closure_supported,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn define_function(
        &mut self,
        code: Id,
        scope: Id,
        name: Id,
        defaults: Id,
        qualname: Id,
        doc: Id,
        annotate: Id,
        annotation_scope: bool,
    ) -> Id {
        let owner = self.scope_data(scope);
        let class_cell = owner.class_cell;
        let scope =
            if owner.exec_locals.is_some() || (owner.class_body.is_some() && !annotation_scope) {
                owner.parent.unwrap_or(scope)
            } else {
                scope
            };
        let fields =
            super::function_fields::FunctionFields::new(self, scope, name, qualname, doc, annotate);
        let signature = self.code_object(code).parameters;
        let signature: serde_json::Value = if signature == NONE {
            serde_json::Value::Null
        } else {
            serde_json::from_str(
                self.str_value(signature)
                    .expect("function parameters are text"),
            )
            .expect("function parameters are valid JSON")
        };
        let entries = self
            .dict_entries(defaults)
            .expect("function defaults are a dict")
            .to_vec();
        let lookup = |name: &str| {
            entries.iter().find_map(|&(key, value)| {
                matches!(self.heap.get(key), Value::Str(text) if text == name).then_some(value)
            })
        };
        let mut positional = Vec::new();
        for key in ["posonly", "positional"] {
            for name in signature[key].as_array().into_iter().flatten() {
                if let Some(name) = name.as_str()
                    && let Some(value) = lookup(name)
                {
                    positional.push(value);
                }
            }
        }
        let keyword: Vec<(Id, Id)> = entries
            .into_iter()
            .filter(|&(key, _)| {
                let Value::Str(name) = self.heap.get(key) else {
                    return false;
                };
                signature["kwonly"]
                    .as_array()
                    .is_some_and(|names| names.iter().any(|item| item.as_str() == Some(name)))
            })
            .collect();
        let defaults = if positional.is_empty() {
            NONE
        } else {
            self.alloc(Value::Tuple(positional))
        };
        let kwdefaults = if keyword.is_empty() {
            NONE
        } else {
            self.dict(keyword)
        };
        let names = self.code_object(code).freevars.clone();
        let mut cells = Vec::new();
        for name in &names {
            let mut cursor = scope;
            loop {
                let frame = self.scope_data(cursor);
                if let Some(&cell) = frame.closure.cells.get(name) {
                    cells.push(cell);
                    break;
                }
                if frame.class_body.is_none() && frame.closure.declared.contains(name) {
                    let Value::Scope(frame) = self.heap.get_mut(cursor) else {
                        unreachable!()
                    };
                    let value = frame.names.shift_remove(name);
                    let cell = self.alloc(Value::Cell(value));
                    self.heap.keep(cell);
                    let Value::Scope(frame) = self.heap.get_mut(cursor) else {
                        unreachable!()
                    };
                    frame.closure.cells.insert(name.clone(), cell);
                    cells.push(cell);
                    break;
                }
                cursor = frame.parent.expect("free variable has enclosing scope");
            }
        }
        let scope = if !annotation_scope && self.code_object(code).closure_supported {
            let mut globals = scope;
            while let Some(parent) = self.scope_data(globals).parent {
                globals = parent;
            }
            self.closure_environment(globals, names, &cells)
        } else {
            scope
        };
        let closure = if cells.is_empty() {
            NONE
        } else {
            self.alloc(Value::Tuple(cells))
        };
        self.alloc(Value::Function {
            fields,
            closure,
            code,
            scope,
            class_cell,
            defaults,
            kwdefaults,
        })
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_failed(ctx: *const Context) -> bool {
    // SAFETY: the generated caller holds the context throughout this query.
    unsafe { (*ctx).pending.is_some() }
}

/// Record the caller's temporary root boundary before an expression runs.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_truth_mark(ctx: *const Context) -> usize {
    // SAFETY: the generated caller owns this context for the entire expression.
    unsafe { (&*ctx).heap.temporary_mark() }
}

/// Release a consumed truth operand on the branch that no longer needs it.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_discard_truth(ctx: *mut Context, mark: usize) {
    // SAFETY: the mark comes from the same active Python frame.
    unsafe { (&mut *ctx).discard_truth(mark) }
}

/// Publish the named expression's result before the caller evaluates its next operand.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_named_assignment(
    ctx: *mut Context,
    mark: usize,
    value: Id,
    replaced: Id,
) {
    // SAFETY: the mark and result belong to the current generated expression.
    let ctx = unsafe { &mut *ctx };
    if replaced == TRUE && ctx.heap.named_assignment(mark, value) {
        ctx.collect_unreachable();
        ctx.heap.release_unbound_writers();
    }
}

/// Borrow the explicitly live handles in the generated caller's temporary array.
unsafe fn live_handles<'a>(live: *const Id, len: usize) -> &'a [Id] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: the native caller provides an array of len handles for this call.
        unsafe { slice::from_raw_parts(live, len) }
    }
}

/// Finish one target before the next target can observe a replaced writer.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_assignment_target(
    ctx: *mut Context,
    live: *const Id,
    len: usize,
    replaced: Id,
) {
    // SAFETY: the caller owns the context and its live handle array.
    let (ctx, live) = unsafe { (&mut *ctx, live_handles(live, len)) };
    if ctx.heap.assignment_target(live, replaced == TRUE) {
        ctx.collect_unreachable();
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_statement(
    ctx: *mut Context,
    live: *const Id,
    len: usize,
    file: *const c_char,
    line: u32,
) {
    // SAFETY: older emitters supplied the same live roots and static file name.
    unsafe { imre_dyn_statement_with_exception(ctx, live, len, file, line, NONE) };
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_statement_with_exception(
    ctx: *mut Context,
    live: *const Id,
    len: usize,
    file: *const c_char,
    line: u32,
    caught: Id,
) {
    // SAFETY: the emitter supplies its current frame's explicitly live temporaries.
    let (ctx, live) = unsafe { (&mut *ctx, live_handles(live, len)) };
    // SAFETY: file names are static constants in the generated native program.
    let file = unsafe { cstr(file) };
    ctx.checkpoint(
        live,
        value::Location {
            file: file.into(),
            line,
            span: None,
        },
        caught,
    );
}

impl Context {
    /// Check the old binding without calling a Python mapping's lookup hook.
    pub(super) fn replacing_writer(&self, scope: Id, name: &str, value: Id) -> bool {
        if !self.heap.has_file_writers() {
            return false;
        }
        let scope = self.binding_scope(scope, name);
        let data = self.scope_data(scope);
        data.exec_locals.is_some()
            || data.exec_globals.is_some()
            || self
                .scope_name(scope, name)
                .is_some_and(|old| old != value && self.heap.could_own_writer(old, false))
    }

    pub(super) fn heap_and_roots(&mut self) -> (&mut Heap, impl Iterator<Item = Id> + '_) {
        (
            &mut self.heap,
            self.modules
                .values()
                .chain(self.classes.values())
                .chain(self.logger_wrappers.values())
                .chain(self.typing.roots())
                .chain(self.builtins.values())
                .chain(self.native_code_objects.values())
                .chain(self.codec_entries.roots())
                .copied()
                .chain(self.contexts.iter().copied())
                .chain(self.bytearray_exports.iter().copied())
                .chain(self.pending.iter().flat_map(Error::roots))
                .chain(self.source_builtins)
                .chain([
                    self.argv,
                    self.sys_path,
                    self.default_encoding,
                    self.token_missing,
                ]),
        )
    }

    pub(super) fn checkpoint(&mut self, live: &[Id], location: value::Location, caught: Id) {
        let (heap, roots) = self.heap_and_roots();
        if let Some(callbacks) = heap.statement(roots, live, location, caught) {
            self.run_weak_callbacks(callbacks);
        }
    }

    /// Run deferred callbacks when a native loop reaches a collection point.
    pub(super) fn collect_garbage(&mut self) {
        let (heap, roots) = self.heap_and_roots();
        if let Some(callbacks) = heap.collect_if_due(roots) {
            self.run_weak_callbacks(callbacks);
        }
    }

    /// Flush unreachable file owners before the caller evaluates its next operand.
    pub(super) fn leave_frame(&mut self, result: Id, returning: bool) {
        if self.heap.leave(result, returning) {
            self.collect_unreachable();
        }
    }

    /// Publish the surviving roots before the next operand can observe a discarded writer.
    pub(super) fn discard_truth(&mut self, mark: usize) {
        self.finish_call_operands(mark, NONE);
    }

    /// Keep the returned value and earlier operands while dropping the completed call's temporaries.
    pub(super) fn finish_call_operands(&mut self, mark: usize, result: Id) {
        if self.heap.discard_operands(mark, result) {
            self.collect_unreachable();
        }
    }

    /// Sweep using the active frames and the context's roots before running callbacks.
    pub(super) fn collect_unreachable(&mut self) {
        let (heap, roots) = self.heap_and_roots();
        if let Some(callbacks) = heap.collect(roots) {
            self.run_weak_callbacks(callbacks);
        }
    }

    /// Invoke pending callbacks after the heap has cleared every dead reference.
    fn run_weak_callbacks(&mut self, callbacks: Vec<(Id, Id)>) {
        for (callback, reference) in callbacks {
            if let Err(error) = self.call(callback, vec![reference], Vec::new()) {
                eprintln!("Exception ignored while calling weakref callback: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::{Rc, Weak};

    #[test]
    fn surviving_function_and_cell_trace_only_the_captured_binding() {
        unsafe extern "C" fn read(_: *mut Context, _: Id) -> Id {
            NONE
        }

        fn payload(ctx: &mut Context) -> (Id, Weak<[Vec<String>]>) {
            let value: Rc<[Vec<String>]> = Rc::from([]);
            let probe = Rc::downgrade(&value);
            (ctx.alloc(Value::AttrGetter(value)), probe)
        }

        let argv = [c"imre".as_ptr()];
        // SAFETY: the arguments and configuration outlive initialization, which returns an owned context.
        let mut ctx =
            unsafe { Box::from_raw(imre_dyn_new(1, argv.as_ptr(), c"{\"paths\":[]}".as_ptr())) };
        let globals = ctx.scope("example", None);
        let outer = ctx.scope("example", Some(globals));
        let Value::Scope(frame) = ctx.heap.get_mut(outer) else {
            unreachable!()
        };
        frame
            .closure
            .declared
            .extend(["kept".into(), "unused".into()]);
        frame.closure.declared_complete = true;
        let (kept, kept_probe) = payload(&mut ctx);
        let (unused, unused_probe) = payload(&mut ctx);
        ctx.bind(outer, "kept", kept);
        ctx.bind(outer, "unused", unused);
        let name = ctx.text("read");
        let signature = ctx.text(r#"{"scope_complete":true,"scope_reads":["kept"]}"#);
        let location = value::Location {
            file: "cells.py".into(),
            line: 1,
            span: None,
        };
        let code = ctx.prelinked_code(read, outer, name, signature, (name, NONE), location.clone());
        let defaults = ctx.dict(Vec::new());
        let function = ctx.define_function(code, outer, name, defaults, name, NONE, NONE, false);
        let closure = ctx.lookup_attr(function, "__closure__").unwrap();
        let Value::Tuple(cells) = ctx.heap.get(closure) else {
            unreachable!()
        };
        let cell = cells[0];
        for (roots, keeps_value) in [
            (&[function][..], true),
            (&[cell][..], true),
            (&[][..], false),
        ] {
            // Force a statement collection even after the previous sweep populated free slots.
            for _ in 0..16384 {
                ctx.alloc(Value::None);
            }
            ctx.checkpoint(roots, location.clone(), NONE);
            assert!(unused_probe.upgrade().is_none());
            assert_eq!(kept_probe.upgrade().is_some(), keeps_value);
        }
    }
}
