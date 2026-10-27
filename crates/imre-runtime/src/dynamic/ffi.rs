//! ABI between LLVM-generated Python functions and Imre-owned objects.

use std::ffi::{CStr, c_char};
use std::slice;

use super::*;

fn flag(v: bool) -> Id {
    if v { TRUE } else { FALSE }
}

impl Context {
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
                    self.exception_class_repr(value),
                    self.exception_class_repr(actual)
                ),
            ));
        }
        Ok(result)
    }

    /// Format exception-construction diagnostics with the class's Python module.
    fn exception_class_repr(&self, class: Id) -> String {
        let Value::Class { name, scope, .. } = self.heap.get(class) else {
            return self.repr(class);
        };
        let scope = self.scope_data(*scope);
        let attr = |key: &str| {
            scope
                .names
                .get(key)
                .and_then(|&id| match self.heap.get(id) {
                    Value::Str(s) => Some(s.as_str()),
                    _ => None,
                })
        };
        let module = attr("__module__").unwrap_or(&scope.module);
        let name = attr("__qualname__").unwrap_or(name);
        if module == "builtins" {
            format!("<class '{name}'>")
        } else {
            format!("<class '{module}.{name}'>")
        }
    }

    pub(super) fn op(&mut self, op: &str, a: Id, b: Id, c: Id) -> Result<Id> {
        match op {
            "lookup" => {
                let name = self.str_value(b)?.to_owned();
                self.lookup(a, &name)
            }
            "assign" => {
                let name = self.str_value(b)?.to_owned();
                let value = self.enum_assignment(a, &name, c)?;
                self.bind_source_name(a, &name, value)?;
                Ok(c)
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
                    return Err(Error::new(
                        "NameError",
                        format!("name '{name}' is not defined"),
                    ));
                }
                Ok(NONE)
            }
            "scope" => {
                let module = self.scope_data(a).module.clone();
                Ok(self.scope(&module, Some(a)))
            }
            "import" => {
                let name = self.str_value(b)?.to_owned();
                self.module(&name)
            }
            "from" => self.import_from(a, b),
            "attr" => {
                let name = self.str_value(b)?.to_owned();
                self.lookup_attr(a, &name)
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
                    Value::List(v) => v.push(b),
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
                let values = self.items(a)?;
                let count = usize::try_from(self.as_integer(b)?)
                    .map_err(|_| Error::new("ValueError", "invalid unpack length"))?;
                if c == NONE {
                    if count != values.len() {
                        return Err(Error::new("ValueError", "wrong number of values to unpack"));
                    }
                    Ok(self.alloc(Value::List(values)))
                } else {
                    let star = usize::try_from(self.as_integer(c)?)
                        .map_err(|_| Error::new("ValueError", "invalid starred target"))?;
                    if star >= count || values.len() < count - 1 {
                        return Err(Error::new("ValueError", "not enough values to unpack"));
                    }
                    let tail = values.len() - (count - star - 1);
                    let mut out = values[..star].to_vec();
                    out.push(self.alloc(Value::List(values[star..tail].to_vec())));
                    out.extend(&values[tail..]);
                    Ok(self.alloc(Value::List(out)))
                }
            }
            "slice_object" => Ok(self.alloc(Value::Slice([a, b, c]))),
            "call" => {
                let args = self.items(b)?;
                let kwargs = self.keyword_args(c)?;
                self.call(a, args, kwargs)
            }
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
            "generator" => self.generator(a, b),
            "yielded" => Ok(self.alloc(Value::Yielded(a))),
            "iter" => self.loop_iterator(a),
            "more" => Ok(flag(self.iterator_has_item(a)?)),
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
            "format" => {
                let mut value = a;
                if b != NONE {
                    let conv = self.str_value(b)?;
                    let s = match conv {
                        "r" => self.repr(a),
                        "s" => self.display(a),
                        "a" => self
                            .repr(a)
                            .chars()
                            .flat_map(|c| {
                                if c.is_ascii() {
                                    c.to_string()
                                } else if (c as u32) <= 0xff {
                                    format!("\\x{:02x}", c as u32)
                                } else if (c as u32) <= 0xffff {
                                    format!("\\u{:04x}", c as u32)
                                } else {
                                    format!("\\U{:08x}", c as u32)
                                }
                                .chars()
                                .collect::<Vec<_>>()
                            })
                            .collect(),
                        _ => return Err(Error::new("ValueError", "unsupported conversion")),
                    };
                    value = self.text(s);
                }
                let spec = if c == NONE {
                    String::new()
                } else {
                    self.str_value(c)?.to_owned()
                };
                let s = builtins::format_value(self, value, &spec)?;
                Ok(self.text(s))
            }
            "take_error" => {
                let error = self
                    .pending
                    .take()
                    .ok_or_else(|| Error::new("RuntimeError", "no active exception"))?;
                Ok(self.raised_instance(error))
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
            "exit_exception" => {
                let class = self.exact_type(b)?;
                let traceback = self.exception_traceback(b)?;
                let prior = self.heap.replace_active(b);
                let result = self.call(a, vec![class, b, traceback], Vec::new());
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
                                    self.exception_class_repr(a),
                                    self.type_name(normalized)
                                ),
                            ));
                        };
                        error = result.clone();
                        error.tracing().instance = Some(normalized);
                    }
                    let active = self.heap.active_exception();
                    if active != NONE && error.tracing().instance != Some(active) {
                        error.tracing().context = Some(active);
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
        contexts: Vec::new(),
        bytearray_exports: Vec::new(),
        sorting_lists: Vec::new(),
        repr_ancestors: Vec::new(),
        token_missing: NONE,
        native_code_objects: HashMap::new(),
        source_builtins: None,
        codec_entries: Default::default(),
        modules: HashMap::new(),
        classes: HashMap::new(),
        typing: super::typing::Registry::default(),
        logger_wrappers: HashMap::new(),
        builtins: HashMap::new(),
        sources: HashMap::new(),
        call_depth: 0,
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
        Err(e) => {
            eprintln!("{e}");
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
    let value = match kind {
        0 => std::str::from_utf8(data)
            .ok()
            .and_then(crate::parse_integer_literal)
            .map(crate::Int::new)
            .map(Value::Int)
            .ok_or_else(|| Error::new("ValueError", "invalid integer literal")),
        1 => std::str::from_utf8(data)
            .map(|s| Value::Str(s.to_owned()))
            .map_err(|e| Error::new("UnicodeError", e.to_string())),
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
                    if frame.parent.is_none() {
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
        for name in names {
            let mut cursor = scope;
            loop {
                let frame = self.scope_data(cursor);
                if let Some(&cell) = frame.closure.cells.get(&name) {
                    cells.push(cell);
                    break;
                }
                if frame.class_body.is_none() && frame.closure.declared.contains(&name) {
                    let cell = self.alloc(Value::Cell {
                        scope: cursor,
                        name: name.clone(),
                    });
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
    let ctx = unsafe { &mut *ctx };
    let live = if len == 0 {
        &[]
    } else {
        // SAFETY: the native frame owns this array of len live handles.
        unsafe { slice::from_raw_parts(live, len) }
    };
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
    pub(super) fn checkpoint(&mut self, live: &[Id], location: value::Location, caught: Id) {
        let Some(callbacks) = self.heap.statement(
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
                .chain([self.argv, self.sys_path, self.token_missing]),
            live,
            location,
            caught,
        ) else {
            return;
        };
        for (callback, reference) in callbacks {
            if let Err(error) = self.call(callback, vec![reference], Vec::new()) {
                eprintln!("Exception ignored while calling weakref callback: {error}");
            }
        }
    }
}
