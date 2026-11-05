//! ABI between LLVM-generated Python functions and Imre-owned objects.

use std::ffi::{CStr, c_char};
use std::slice;

use super::*;

fn flag(v: bool) -> Id {
    if v { TRUE } else { FALSE }
}

impl Context {
    fn linked_constant<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let old = std::mem::replace(&mut self.heap.loading_linked_constant, true);
        let value = build(self);
        self.heap.loading_linked_constant = old;
        value
    }

    /// Call with packed operands and drop their temporary roots after a successful return.
    fn call_packed(
        &mut self,
        callable: Id,
        arguments: Id,
        keywords: Id,
        mark: usize,
        scope: Id,
    ) -> Result<Id> {
        let args = self.items(arguments)?;
        let kwargs = if keywords == NONE {
            Vec::new()
        } else {
            self.keyword_args(keywords)?
        };
        let result = self.call_in_scope(scope, callable, args, kwargs)?;
        if !self
            .heap
            .discard_scalar_call(mark, result, arguments, callable)
        {
            self.finish_call_operands(mark, result);
        }
        self.check_call_return(result)
    }

    /// A Python call expression checks for delivery after the callee's native work completes.
    pub(super) fn check_call_return(&mut self, result: Id) -> Result<Id> {
        #[cfg(unix)]
        if crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Relaxed)
            && crate::signals::is_main()
        {
            // A Python handler can delete the last other reference and force collection.
            self.heap.keep(result);
            self.dispatch_signals()?;
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
        if !self
            .pattern_flags(subject)
            .has(super::collection_pattern::PatternFlags::MAPPING)
        {
            return Ok(FALSE);
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

    /// Class patterns require a type before performing Python's instance check.
    pub(super) fn class_pattern_matches(&mut self, subject: Id, class: Id) -> Result<bool> {
        if !self.is_type_object(class) {
            return Err(Error::new(
                "TypeError",
                "called match pattern must be a class",
            ));
        }
        self.python_instance_check(subject, class)
    }

    /// Fetch keyword fields in source order before evaluating their nested patterns.
    fn match_class(&mut self, subject: Id, class: Id, names: Id) -> Result<Id> {
        if !self.class_pattern_matches(subject, class)? {
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
            "annotation_format" => {
                let limit = self.int(2);
                let unsupported = self.binary(">", a, limit)?;
                if self.truth(unsupported)? {
                    let error = self.builtin("NotImplementedError");
                    self.op("raise", error, NONE, FALSE)
                } else {
                    Ok(NONE)
                }
            }
            "type_alias" => {
                let parameters = if c == NONE {
                    self.heap.packed_tuple(Vec::new(), false)
                } else {
                    c
                };
                Ok(self.alloc(Value::TypeAlias {
                    name: a,
                    parameters,
                    definition: typing::AliasDefinition::Lazy {
                        evaluator: b,
                        cached: None,
                    },
                }))
            }
            "template_interpolation" => {
                let (Value::List(meta) | Value::Tuple(meta)) = self.heap.get(c) else {
                    unreachable!("compiled interpolation metadata");
                };
                let &[conversion, spec] = meta.as_slice() else {
                    unreachable!("compiled interpolation metadata length");
                };
                self.template_interpolation(a, b, conversion, spec)
            }
            "build_template" => {
                let interpolations = self.items(b)?;
                Ok(self.template_from_literal(a, interpolations))
            }
            "linked_string_tuple" => {
                let items = self.items(a)?;
                Ok(self.linked_constant(|ctx| {
                    ctx.heap
                        .linked_string_constant(b, |heap| heap.packed_tuple(items, true))
                }))
            }
            "linked_string" => Ok(self.heap.linked_string_constant(b, |_| a)),
            "match_class" => self.match_class(a, b, c),
            "match_mapping" => self.match_mapping(a, b),
            "match_mapping_keys" => self.match_mapping_keys(a, b),
            "match_sequence" => Ok(builtins::boolean(
                self.pattern_flags(a)
                    .has(super::collection_pattern::PatternFlags::SEQUENCE),
            )),
            "match_sequence_length" => builtins::call(self, "len", &[a], &[]),
            "builtin" => {
                let name = copy_label(self.str_value(a)?)?;
                Ok(self.builtin(&name))
            }
            "annotation_scope" => Ok(self
                .scope_data(a)
                .parent
                .expect("annotation function scope")),
            "lookup_annotation_global"
            | "lookup_class_annotation"
            | "lookup_class_annotation_global" => {
                let name = copy_label(self.str_value(b)?)?;
                if op != "lookup_annotation_global" {
                    if let Some(&cell) = self.scope_data(a).closure.cells.get("__classdict__") {
                        let Value::Cell(Some(mapping)) = *self.heap.get(cell) else {
                            return Err(self.missing_binding(a, a, "__classdict__"));
                        };
                        match self.load_exec_name(mapping, &name) {
                            Err(error) if self.is_builtin_error(&error, "KeyError") => {}
                            value => return value,
                        }
                    } else if let Some(value) = self.scope_name(a, &name) {
                        return Ok(value);
                    }
                }
                let scope = if op == "lookup_class_annotation" {
                    a
                } else {
                    self.root_scope(a)
                };
                self.lookup(scope, &name)
            }
            "lookup" | "lookup_class_local" => {
                let name = copy_label(self.str_value(b)?)?;
                if op == "lookup_class_local" {
                    return self.lookup_class_local(a, &name);
                }
                self.lookup(a, &name)
            }
            "assign" | "assign_target" | "assign_named_target" => {
                let scope = if op == "assign_named_target" {
                    self.named_expression_scope(a)
                } else {
                    a
                };
                let name = copy_label(self.str_value(b)?)?;
                let replaced = self.store_source_name(scope, &name, c)?;
                if matches!(op, "assign_target" | "assign_named_target") {
                    Ok(flag(replaced))
                } else {
                    if replaced {
                        self.heap.release_unbound_finalizers();
                    }
                    Ok(c)
                }
            }
            "declare_global" => {
                let name = copy_label(self.str_value(b)?)?;
                let Value::Scope(scope) = self.heap.get_mut(a) else {
                    unreachable!()
                };
                scope.globals.insert(name);
                Ok(NONE)
            }
            "delname" | "clearname" => {
                let name = copy_label(self.str_value(b)?)?;
                let released = self.replacing_finalizer(a, &name, NONE);
                let scope = self.binding_scope(a, &name);
                if self.remove_binding(scope, &name, b)? {
                    if released {
                        self.heap.release_unbound_finalizers();
                    }
                } else if op == "delname" {
                    return Err(self.missing_binding(a, scope, &name));
                }
                Ok(NONE)
            }
            "comprehension_scope" => self.comprehension_scope(a, b),
            "import" => {
                let name = copy_label(self.str_value(b)?)?;
                self.module(&name)
            }
            "fromlist" => self.prepare_fromlist(a, b),
            "from" => self.import_from(a, b),
            "import_star" => self.import_star(a, b),
            "lookup_enter" => self.context_method(a, "__enter__"),
            "lookup_exit" => self.context_method(a, "__exit__"),
            "attr" | "method" => {
                let name = copy_label(self.str_value(b)?)?;
                if op == "method" {
                    self.lookup_method(a, &name)
                } else {
                    self.lookup_attr(a, &name)
                }
            }
            "setattr" => {
                let name = copy_label(self.str_value(b)?)?;
                self.set_attr(a, &name, c)?;
                Ok(c)
            }
            "delattr" => {
                let name = copy_label(self.str_value(b)?)?;
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
            "spread_arguments" => {
                // An exact tuple already holds the evaluated positional arguments.
                // Snapshot other iterables before entering the callee.
                if matches!(self.heap.get(a), Value::Tuple(_)) {
                    Ok(a)
                } else {
                    let items = self.items(a)?;
                    Ok(self.alloc(Value::List(items)))
                }
            }
            "set" => Ok(self.alloc(Value::Set {
                entries: Default::default(),
                frozen: false,
            })),
            "dict" => Ok(self.dict(Vec::new())),
            "tuple" | "constant_tuple" => {
                let items = self.items(a)?;
                Ok(if op == "constant_tuple" {
                    self.linked_constant(|ctx| ctx.heap.packed_tuple(items, true))
                } else {
                    self.heap.packed_tuple(items, false)
                })
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
                let items = self.items(b)?;
                if let Value::List(list) = self.heap.get_mut(a) {
                    // Fill the available space before growing so __sizeof__ and
                    // partial progress on allocation failure match repeated append.
                    let mut remaining = items.as_slice();
                    while !remaining.is_empty() {
                        list.try_reserve(1)
                            .map_err(|_| Error::new("MemoryError", ""))?;
                        let count = remaining.len().min(list.capacity() - list.len());
                        let (chunk, rest) = remaining.split_at(count);
                        list.extend_from_slice(chunk);
                        remaining = rest;
                    }
                } else {
                    for item in items {
                        self.op("append", a, item, NONE)?;
                    }
                }
                Ok(a)
            }
            "update" => self.merge_mapping(a, b, super::calls::MappingOperation::Unpack),
            "keywords" => {
                self.merge_mapping(a, b, super::calls::MappingOperation::Keywords(Some(c)))
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
            "class_typevartuple" => Ok(super::typing::declared_pack(self, a)),
            "class_paramspec" => Ok(super::paramspec::declared(self, a)),
            "class_parameter_scope" | "function_parameter_scope" => {
                super::typing::parameter_scope(self, a, b, op == "class_parameter_scope")
            }
            "class_annotation_environment" => self.class_annotation_environment(a, b),
            "class_annotation_locals" => {
                let parent = self.scope_data(a).parent.expect("annotation globals");
                let local = self.scope("", Some(parent));
                let Value::Scope(frame) = self.heap.get_mut(local) else {
                    unreachable!()
                };
                frame.exec_locals = Some(b);
                Ok(local)
            }
            "class_base_scope" => {
                if self.scope_data(b).class_body.is_none() {
                    return Ok(a);
                }
                // A generic class's bases can read its enclosing class dictionary. Its
                // body and methods close over the parameter scope without that dictionary.
                let module = self.scope_data(a).module.clone();
                let scope = self.scope(&module, Some(a));
                let namespace = self.class_namespace_dict(b, true);
                let Value::Scope(frame) = self.heap.get_mut(scope) else {
                    unreachable!()
                };
                frame.exec_locals = Some(namespace);
                frame.closure.declared_complete = true;
                Ok(scope)
            }
            "class" | "parameterized_class" => {
                let keywords = if a == NONE {
                    Vec::new()
                } else {
                    self.keyword_args(a)?
                };
                self.class(b, c, keywords, op == "parameterized_class")
            }
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
            "mark_module_annotation" => {
                self.mark_module_annotation(a, b)?;
                Ok(NONE)
            }
            "setup_module_string_annotations" | "setup_module_annotations" => {
                self.setup_module_annotations(a, op == "setup_module_string_annotations")?;
                Ok(NONE)
            }
            "annotate" => {
                if self.scope_data(a).class_body.is_some() {
                    let mapping = self.lookup_class_local(a, "__annotations__")?;
                    self.put_item(mapping, b, c)?;
                } else {
                    let mapping = self.module_annotation_name(a, "__annotations__")?;
                    self.put_item(mapping, b, c)?;
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
                    self.heap.release_unbound_finalizers();
                }
                Ok(flag(more))
            }
            "next" => self
                .next_item(a)?
                .ok_or_else(|| Error::new("StopIteration", "")),
            "not" => Ok(flag(!self.truth(a)?)),
            "neg" | "pos" if matches!(self.heap.get(a), Value::Calendar { value, .. } if value.class() == "timedelta") =>
            {
                let method = if op == "neg" {
                    "datetime.timedelta.__neg__"
                } else {
                    "datetime.timedelta.__pos__"
                };
                stdlib::call(self, method, &[a], &[])
            }
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
                self.heap.release_unbound_finalizers();
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
            "exit_context" => {
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
    crate::initialize_io();
    #[cfg(unix)]
    crate::signals::initialize_interrupt();
    // SAFETY: native main supplies argc/argv and a static JSON constant.
    let mut config: serde_json::Value =
        serde_json::from_str(unsafe { cstr(config) }).expect("compiler target metadata");
    if config["no_site"] != true {
        // site normally caches this value before the entry can change its environment.
        config["initial_user_base"] = imre_ast::layout::InterpreterLayout::user_base()
            .and_then(|base| base.into_os_string().into_string().ok())
            .into();
    }
    let ignore_environment = config["ignore_environment"].as_bool().unwrap_or(false)
        || config["isolated"].as_bool().unwrap_or(false);
    if config["faulthandler"].is_null() {
        config["faulthandler"] = (!ignore_environment
            && std::env::var_os("PYTHONFAULTHANDLER").is_some_and(|value| !value.is_empty()))
        .into();
    }
    let options: Vec<String> =
        serde_json::from_value(config["warnoptions"].take()).unwrap_or_default();
    config["warnoptions"] = crate::sys::startup_warning_options(ignore_environment, options).into();
    if config["unbuffered"].as_bool() == Some(true) {
        crate::imre_stdio_unbuffered_init();
    }
    crate::sys::initialize_encoding(ignore_environment, config["utf8_mode"].as_bool());
    config["utf8_mode"] = crate::sys::imre_sys_utf8_mode().into();
    config["warn_default_encoding"] = config["warn_default_encoding"]
        .as_bool()
        .unwrap_or_else(|| {
            !ignore_environment
                && std::env::var_os("PYTHONWARNDEFAULTENCODING")
                    .is_some_and(|value| !value.is_empty())
        })
        .into();
    if !ignore_environment {
        config["pycache_prefix"] = std::env::var("PYTHONPYCACHEPREFIX")
            .ok()
            .filter(|prefix| !prefix.is_empty())
            .into();
    }
    crate::sys::imre_process_config_init(
        u32::from(ignore_environment),
        config["cpu_count"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .unwrap_or(if ignore_environment { 0 } else { -1 }),
        config["context_aware_warnings"]
            .as_bool()
            .map_or(if ignore_environment { 0 } else { -1 }, i32::from),
    );
    if let Some(options) = config["warnoptions"].as_array() {
        for option in options.iter().filter_map(|value| value.as_str()) {
            // Keep the native typed fallback in sync. The public warnings module
            // diagnoses invalid options once, using the full Python category lookup.
            let _ = crate::objects::warnings::startup_filter(option);
        }
    }
    // SAFETY: main supplies argc pointers to NUL-terminated strings that remain
    // valid while the constructor consumes this iterator.
    let args = unsafe { slice::from_raw_parts(argv, argc as usize) }
        .iter()
        .map(|&s| unsafe { CStr::from_ptr(s) }.to_string_lossy());
    Box::into_raw(Box::new(new_context(config, args)))
}

pub(super) fn new_context<S: Into<String>>(
    config: serde_json::Value,
    args: impl IntoIterator<Item = S>,
) -> Context {
    let heap = Heap::new();
    if let Some(depth @ 1..=65535) = config["tracemalloc"].as_u64() {
        heap.allocation_traces.start_at_startup(depth as usize);
    }
    new_context_with_heap(config, args, heap)
}

pub(super) fn new_context_with_heap<S: Into<String>>(
    config: serde_json::Value,
    args: impl IntoIterator<Item = S>,
    heap: Heap,
) -> Context {
    let launcher = config["executable"].as_str();
    let helper_executable =
        launcher.map_or_else(crate::objects::python_executable_text, str::to_owned);
    let executable = launcher
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_default();
    let interpreter_layout = stdlib::interpreter_layout(&config, &executable);
    let mut ctx = Context {
        heap,
        interpreter_layout,
        threads: Default::default(),
        workers: Default::default(),
        socket_default_timeout: Default::default(),
        config,
        executable,
        helper_executable,
        argv: NONE,
        sys_path: NONE,
        default_encoding: NONE,
        audit_hooks: Vec::new(),
        fork_hooks: Default::default(),
        fault_file: None,
        exit_callbacks: Vec::new(),
        finalizing: Default::default(),
        contexts: Vec::new(),
        buffer_exports: Vec::new(),
        sorting_lists: Vec::new(),
        repr_ancestors: Vec::new(),
        // Measure the execution thread before entering the native Python evaluator.
        repr_sequence_words: super::operators::remaining_repr_words(),
        suspended_typing_frames: 0,
        token_missing: NONE,
        native_code_objects: HashMap::new(),
        original_builtins: None,
        source_builtins: None,
        zip_directories: HashMap::new(),
        zip_path_importers: NONE,
        codec_entries: Default::default(),
        warning_state: None,
        frozen_override: 0,
        modules: NONE,
        import_missing: NONE,
        bootstrap_globals: NONE,
        bootstrap_module: NONE,
        import_registry_installed: false,
        bootstrap_file_spec: NONE,
        finder_external: NONE,
        classes: HashMap::new(),
        enum_scopes: HashMap::new(),
        signal_handlers: Vec::new(),
        typing: super::typing::Registry::default(),
        builtins: HashMap::new(),
        native_type_dicts: HashMap::new(),
        sources: HashMap::new(),
        call_depth: 0,
        deque_comparison_repeats: 0,
        pending: None,
        slot_depth: 0,
    };
    ctx.modules = ctx.dict(Vec::new());
    let object = ctx.builtin("object");
    ctx.import_missing = ctx.instance(object, None);
    let args = args.into_iter().map(|s| ctx.text(s)).collect();
    ctx.argv = ctx.alloc(Value::List(args));
    let paths: Vec<String> =
        serde_json::from_value(ctx.config["paths"].clone()).expect("compiler search paths");
    // The CLI adds its script directory only after site has finished.
    let defer_path0 = ctx.config["sitecustomize"] == true && ctx.config["path0"] == true;
    let paths = paths
        .into_iter()
        .skip(usize::from(defer_path0))
        .map(|p| ctx.text(p))
        .collect();
    ctx.sys_path = ctx.alloc(Value::List(paths));
    ctx.default_encoding = ctx.heap.code_name(crate::sys::default_encoding().as_str());
    let current = ctx.alloc(Value::Context(Default::default()));
    ctx.contexts.push(current);
    ctx.token_missing = ctx.alloc(Value::TokenMissing);
    // Resolving the handler after an allocation failure must not grow the builtin cache.
    ctx.builtin("MemoryError");
    ctx
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    code: Code,
) {
    // SAFETY: earlier native code supplies the same static names and initializer, without text.
    unsafe { imre_dyn_register_source(ctx, name, file, std::ptr::null(), 0, code) }
}

/// Register a compiled initializer and the exact Python text of a bundled provider.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_source(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    text: *const u8,
    len: u64,
    code: Code,
) {
    // SAFETY: module names and filenames are static compiler-emitted strings.
    let (ctx, name, file) = unsafe { (&mut *ctx, cstr(name), cstr(file)) };
    // SAFETY: non-null text points to a static source constant with exactly len bytes.
    let bundled_text =
        (!text.is_null()).then(|| unsafe { std::slice::from_raw_parts(text, len as usize) });
    register_source(
        ctx,
        name,
        file,
        ModuleInitializer::Native(code),
        bundled_text,
    );
}

/// Register a bundled Python module whose loader compiles its source on request.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_bundled_source(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    text: *const u8,
    len: u64,
) {
    // SAFETY: the compiler supplies static UTF-8 source and module names.
    let (ctx, name, file, bytes) = unsafe {
        (
            &mut *ctx,
            cstr(name),
            cstr(file),
            std::slice::from_raw_parts(text, len as usize),
        )
    };
    let source = std::str::from_utf8(bytes).expect("compiler-emitted Python is UTF-8");
    register_source(
        ctx,
        name,
        file,
        ModuleInitializer::Source(source),
        Some(bytes),
    );
}

fn register_source(
    ctx: &mut Context,
    name: &str,
    file: &str,
    code: ModuleInitializer,
    bundled_text: Option<&'static [u8]>,
) {
    ctx.sources
        .entry(name.to_owned())
        .or_default()
        .push(SourceModule {
            path: file.into(),
            code,
            bundled_text,
        });
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_finish(ctx: *mut Context) -> i32 {
    // SAFETY: main transfers its unique allocation from imre_dyn_new exactly once.
    let mut ctx = unsafe { Box::from_raw(ctx) };
    let mark = ctx.heap.temporary_mark();
    let mut failed_site = false;
    let result = ctx.startup_sys().and_then(|_| {
        if ctx.config["faulthandler"].as_bool() == Some(true) {
            let handler = ctx.module("faulthandler")?;
            let enable = ctx.lookup_attr(handler, "enable")?;
            // CPython enables the handler with its preliminary stderr printer, before
            // exposing the final standard streams. Its descriptor is 2 even when closed.
            let fd = ctx.int(i64::from(libc::STDERR_FILENO));
            ctx.call(enable, vec![fd], Vec::new())?;
        }
        // CPython snapshots dispositions after enabling a startup fault handler.
        #[cfg(unix)]
        ctx.initialize_signal_handlers();
        // The module registry now owns sys. Its startup temporaries must not
        // keep replaced streams alive throughout the main module's execution.
        ctx.heap.release_temporaries_since(mark);
        ctx.install_import_registry()?;
        if ctx.config["warnoptions"]
            .as_array()
            .is_some_and(|options| !options.is_empty())
        {
            let mark = ctx.heap.temporary_mark();
            if let Err(error) = ctx.module("warnings") {
                eprintln!("'import warnings' failed; traceback:");
                stdlib::traceback::report_chain(&mut ctx, error);
            }
            ctx.heap.release_temporaries_since(mark);
        }
        if ctx.config["sitecustomize"] == true {
            let mark = ctx.heap.temporary_mark();
            ctx.initialize_startup_site()
                .inspect_err(|_| failed_site = true)?;
            ctx.heap.release_temporaries_since(mark);
            if ctx.config["path0"] == true {
                ctx.insert_startup_path0()?;
            }
        }
        let main = ctx
            .load_bootstrap_module("__main__", None, NONE)?
            .ok_or_else(|| Context::missing_module("__main__"))?;
        if let Some(name) = ctx.config["module_entry"].as_str().map(str::to_owned) {
            ctx.run_module_as_main(name)
        } else {
            Ok(main)
        }
    });
    if failed_site && let Err(error) = result {
        eprintln!("Fatal Python error: init_import_site: Failed to import the site module");
        eprintln!("Python runtime state: initialized");
        stdlib::traceback::report_chain(&mut ctx, error);
        return 1;
    }
    let mut interrupted = result
        .as_ref()
        .is_err_and(|e| ctx.is_builtin_error(e, "KeyboardInterrupt"));
    let status = match result {
        Ok(_) => 0,
        Err(error) if ctx.is_builtin_error(&error, "SystemExit") => {
            let mark = ctx.heap.temporary_mark();
            let status = report_system_exit(&mut ctx, error);
            ctx.heap.release_temporaries_since(mark);
            status
        }
        Err(error) if let Some(sys) = ctx.threads.system_module => {
            let mark = ctx.heap.temporary_mark();
            let status = super::excepthook::report(&mut ctx, sys, error, &mut interrupted);
            ctx.heap.release_temporaries_since(mark);
            status
        }
        // Startup can fail before the interpreter has created its sys module.
        Err(error) if ctx.is_os_error(&error) && error.args.is_some() => {
            if stdlib::traceback::chain_link(&error).is_some() {
                stdlib::traceback::report_chain(&mut ctx, error);
            } else {
                ctx.report_os_error(error);
            }
            1
        }
        Err(error) if stdlib::traceback::chain_link(&error).is_some() => {
            stdlib::traceback::report_chain(&mut ctx, error);
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
            eprintln!("{}", error.report(&ctx.heap));
            1
        }
    };
    super::thread_worker::shutdown(&mut ctx);
    atexit::run(&mut ctx);
    // Exit callbacks run before Python enters its finalization phase.
    ctx.finalizing
        .store(true, std::sync::atomic::Ordering::Release);
    ctx.heap.discarded_shutdown_write = Some(false);
    ctx.heap.release_temporaries_since(mark);
    if let Err(error) = ctx.release_main_at_shutdown() {
        super::unraisable::report(
            &mut ctx,
            error,
            false,
            None,
            "Exception ignored during main module cleanup",
        );
    }
    if let Err(error) = ctx.finalize_shutdown_resources() {
        super::unraisable::report(
            &mut ctx,
            error,
            false,
            None,
            "Exception ignored during resource cleanup",
        );
    }
    stdlib::io::prepare_writer_shutdown(&ctx);
    crate::fault_handler::disable();
    if status == 0 && ctx.heap.has_pending_callback_write() {
        use std::io::Write;

        let _ = writeln!(
            std::io::stderr().lock(),
            "NotImplementedError: buffered stream has unflushed bytes at shutdown; explicitly flush or close Python-backed writers"
        );
        return 1;
    }
    #[cfg(unix)]
    if interrupted {
        let _ = crate::flush_stdout();
        return crate::signals::exit_interrupt();
    }
    status
}

/// Read the live code at shutdown, including overrides on SystemExit subclasses.
pub(super) fn report_system_exit(ctx: &mut Context, error: Error) -> i32 {
    use std::io::Write;

    let instance = error
        .trace
        .as_ref()
        .and_then(|trace| trace.instance)
        .unwrap_or_else(|| ctx.alloc(Value::Raised(error)));
    ctx.heap.keep(instance);
    let code = ctx.lookup_attr(instance, "code").unwrap_or(instance);
    if code == NONE {
        return 0;
    }
    if let Ok(integer) = ctx.as_int(code) {
        return integer.to_i64().unwrap_or(-1) as i32;
    }
    ctx.heap.keep(code);
    if let Some(file) = ctx
        .sys_binding("stderr")
        .ok()
        .flatten()
        .filter(|&f| f != NONE)
    {
        ctx.heap.keep(file);
        if let Ok(writer) = ctx.lookup_attr(file, "write") {
            ctx.heap.keep(writer);
            if let Ok(text) = builtins::call(ctx, "str", &[code], &[]) {
                let _ = ctx.call(writer, vec![text], Vec::new());
            }
        }
    } else if let Ok(text) = builtins::call(ctx, "str", &[code], &[])
        && let Ok(bytes) = ctx.encode_utf8_codepoints(text, false, "backslashreplace")
    {
        let _ = crate::sys::stderr_writer().write_all(&bytes);
    }
    // The message's __str__ or the stream's write method may have replaced stderr.
    let newline = ctx.text("\n");
    let written = ctx.sys_binding("stderr").and_then(|file| {
        let writer = ctx.lookup_attr(file.unwrap_or(NONE), "write")?;
        ctx.call(writer, vec![newline], Vec::new())
    });
    if written.is_err() {
        let _ = crate::sys::stderr_writer().write_all(b"\n");
    }
    1
}

fn finish(ctx: &mut Context, result: Result<Id>) -> Id {
    finish_at(ctx, result, None)
}

fn finish_at(ctx: &mut Context, result: Result<Id>, span: Option<&[u32; 4]>) -> Id {
    let result = result.and_then(|id| {
        ctx.heap.reserve_temporaries(1)?;
        Ok(id)
    });
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

/// Construct a fresh sequence using the permanent handles in a linked literal.
///
/// # Safety
/// `ctx` must be live and `data` must contain exactly `len` readable bytes
/// encoding valid permanent handles as little-endian pairs.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_permanent_sequence(
    ctx: *mut Context,
    data: *const u8,
    len: usize,
    tuple: bool,
) -> Id {
    // SAFETY: generated code supplies a live context and the exact byte length.
    let (ctx, bytes) = unsafe { (&mut *ctx, slice::from_raw_parts(data, len)) };
    if ctx.pending.is_some() {
        return NONE;
    }
    let (handles, rest) = bytes.as_chunks::<2>();
    debug_assert!(rest.is_empty());
    let mut items = Vec::new();
    if items.try_reserve_exact(handles.len()).is_err() {
        return finish(ctx, Err(Error::new("MemoryError", "")));
    }
    items.extend(
        handles
            .iter()
            .map(|&pair| Id::from(u16::from_le_bytes(pair))),
    );
    let id = if tuple {
        ctx.linked_constant(|ctx| ctx.heap.packed_tuple(items, true))
    } else {
        ctx.alloc(Value::List(items))
    };
    finish(ctx, Ok(id))
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
    ctx.linked_constant(|ctx| load_linked_literal(ctx, kind, data))
}

fn load_linked_literal(ctx: &mut Context, kind: i32, data: &[u8]) -> Id {
    // Small decimal integers already occupy permanent slots and need no temporary root.
    if kind == 0
        && let Ok(text) = std::str::from_utf8(data)
        && let Ok(value) = text.parse::<i64>()
        && let Some(id) = Heap::cached_integer(value)
    {
        return id;
    }
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
                .map_err(|e| Error::new("UnicodeError", e.to_string()))
                .and_then(|text| {
                    if kind == 1 {
                        return ctx.heap.string_literal(text);
                    }
                    if kind == 6 {
                        return Ok(ctx.heap.code_name(text));
                    }
                    for name in text.split('\0') {
                        ctx.heap.code_name(name);
                    }
                    Ok(NONE)
                });
            return finish(ctx, id);
        }
        // The compiler packs non-scalar string literals as little-endian code points.
        7 => {
            let (chunks, remainder) = data.as_chunks::<4>();
            if !remainder.is_empty() {
                return finish(
                    ctx,
                    Err(Error::new("SystemError", "invalid code point literal")),
                );
            }
            let mut points = Vec::new();
            if points.try_reserve_exact(chunks.len()).is_err() {
                return finish(ctx, Err(crate::Error::memory_error().into()));
            }
            for chunk in chunks {
                let point = u32::from_le_bytes(*chunk);
                if point > 0x10ffff {
                    return finish(
                        ctx,
                        Err(Error::new("SystemError", "invalid Python code point")),
                    );
                }
                points.push(point);
            }
            let id = ctx.text_codepoints(points);
            return finish(ctx, Ok(id));
        }
        2 => {
            let mut bytes = Vec::new();
            let result = bytes
                .try_reserve_exact(data.len())
                .map(|()| {
                    bytes.extend_from_slice(data);
                    ctx.heap.bytes_from_data(bytes)
                })
                .map_err(|_| crate::Error::memory_error().into());
            return finish(ctx, result);
        }
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

/// Resolve an identifier from the linked code object in the current Python scope.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_lookup(
    ctx: *mut Context,
    scope: Id,
    name: *const u8,
    len: usize,
) -> Id {
    // SAFETY: the emitter supplies a live context and a constant of exactly len bytes.
    let (ctx, name) = unsafe { (&mut *ctx, slice::from_raw_parts(name, len)) };
    if ctx.pending.is_some() {
        return NONE;
    }
    let result = std::str::from_utf8(name)
        .map_err(|error| Error::new("UnicodeError", error.to_string()))
        .and_then(|name| ctx.lookup(scope, name));
    finish(ctx, result)
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

/// Publish a continued expression's source line without changing its statement's GC roots.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_expression_line(ctx: *mut Context, line: u32) {
    // SAFETY: generated code owns the active context for this frame.
    let ctx = unsafe { &mut *ctx };
    ctx.heap
        .replace_expression(value::ExpressionPosition::Line(line));
}

/// Set the caller's exact range for the immediately following compiled call.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_call_position(
    ctx: *mut Context,
    position: *const [u32; 4],
) {
    // SAFETY: the compiler supplies a static [start line, end line, start column, end column].
    let (ctx, position) = unsafe { (&mut *ctx, *position) };
    ctx.heap.set_call_position(Some(position));
}

/// Invoke a compiled call using the root mark taken before its operands were evaluated.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_scoped_call_at(
    ctx: *mut Context,
    callable: Id,
    args: Id,
    kwargs: Id,
    mark: usize,
    span: *const [u32; 4],
    scope: Id,
) -> Id {
    // SAFETY: the generated caller owns the context and supplies a null or static position.
    let (ctx, span) = unsafe { (&mut *ctx, span.as_ref()) };
    scoped_call(ctx, span, |ctx| {
        ctx.call_packed(callable, args, kwargs, mark, scope)
    })
}

/// Look up an immediate call before evaluating its arguments. Exact string methods
/// can pass their receiver as the first positional operand without allocating a method.
///
/// # Safety
/// The caller supplies the active context, a NUL-terminated ordinary str method symbol, and a
/// writable receiver slot. The slot is initialized on successful lookup.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_string_method_target(
    ctx: *mut Context,
    receiver: Id,
    name: *const c_char,
    bound_receiver: *mut Id,
) -> Id {
    // SAFETY: generated code owns the context and passes a UTF-8 constant and one writable slot.
    let (ctx, name, bound_receiver) = unsafe { (&mut *ctx, cstr(name), &mut *bound_receiver) };
    let target = ctx.string_method_target(receiver, name);
    finish_method_target(ctx, target, bound_receiver)
}

/// Resolve an immediate get call before evaluating arguments that may release its receiver.
///
/// # Safety
/// The caller supplies the active context and a writable receiver slot.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_dict_get_target(
    ctx: *mut Context,
    receiver: Id,
    bound_receiver: *mut Id,
) -> Id {
    // SAFETY: generated code owns the context and passes one writable slot.
    let (ctx, bound_receiver) = unsafe { (&mut *ctx, &mut *bound_receiver) };
    let target = ctx.dict_get_target(receiver);
    finish_method_target(ctx, target, bound_receiver)
}

/// Publish the receiver only after method lookup succeeds.
fn finish_method_target(
    ctx: &mut Context,
    target: Result<(Id, Id)>,
    bound_receiver: &mut Id,
) -> Id {
    let result = target.map(|(callable, receiver)| {
        *bound_receiver = receiver;
        callable
    });
    finish(ctx, result)
}

/// Test a handle against the evaluated elements of a literal tuple.
///
/// # Safety
/// The generated caller roots the operands and provides a readable array of
/// `count` handles for the duration of the comparison.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_contains_literal_tuple(
    ctx: *mut Context,
    item: Id,
    values: *const Id,
    count: usize,
    negate: u32,
) -> Id {
    // SAFETY: the caller owns this context and retains the array through rich comparisons.
    let (ctx, values) = unsafe { (&mut *ctx, live_handles(values, count)) };
    if ctx.pending.is_some() {
        return NONE;
    }
    let mut candidates = values.iter().copied();
    let result = ctx
        .contains_next(item, |_| Ok(candidates.next()))
        .map(|found| flag(found ^ (negate != 0)));
    finish(ctx, result)
}

/// Invoke a prepared method. The leading operand is either the unbound method's
/// receiver or None when lookup returned a bound callable.
///
/// # Safety
/// The positional call ABI applies, with one additional leading receiver slot.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_positional_method_at(
    ctx: *mut Context,
    callable: Id,
    values: *const Id,
    count: usize,
    mark: usize,
    span: *const [u32; 4],
    scope: Id,
) -> Id {
    // SAFETY: the compiler writes and retains every operand through the call.
    let values = unsafe { live_handles(values, count) };
    let args = values.strip_prefix(&[NONE]).unwrap_or(values);
    // SAFETY: the subslice belongs to the same live array and the remaining ABI is identical.
    unsafe {
        imre_dyn_positional_call_at(ctx, callable, args.as_ptr(), args.len(), mark, span, scope)
    }
}

/// Return a single field from an exact string partition, or index the result of a
/// custom method using the ordinary subscription protocol.
///
/// # Safety
/// The active caller roots both prepared operands and supplies null or static positions.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_partition_field_at(
    context: *mut Context,
    callable: Id,
    operands: *const [Id; 2],
    index: u32,
    get_span: *const [u32; 4],
    mark: usize,
    span: *const [u32; 4],
    scope: Id,
) -> Id {
    // SAFETY: the generated call supplies a receiver slot and exactly one argument.
    let (ctx, [receiver, separator]) = unsafe { (&mut *context, *operands) };
    if let Value::Native(symbol) = ctx.heap.get(callable)
        && matches!(symbol.as_str(), "str.partition" | "str.rpartition")
        && matches!(ctx.heap.get(receiver), Value::Str(_))
        && matches!(ctx.heap.get(separator), Value::Str(_))
    {
        let reverse = symbol == "str.rpartition";
        // SAFETY: the call position is null or static.
        return scoped_call(ctx, unsafe { span.as_ref() }, |ctx| {
            let fields = crate::objects::partition_text(
                ctx.str_value(receiver)?,
                ctx.str_value(separator)?,
                reverse,
            )?;
            let selected = match (fields, index) {
                (Some((left, _)), 0) => ctx.text(left.to_owned()),
                (Some(_), 1) => separator,
                (Some((_, right)), 2) => ctx.text(right.to_owned()),
                (None, 0) if !reverse => receiver,
                (None, 2) if reverse => receiver,
                (None, 0..=2) => ctx.text(""),
                _ => unreachable!("compiler emits indices in 0..=2"),
            };
            ctx.finish_call_operands(mark, selected);
            ctx.check_call_return(selected)
        });
    }
    // SAFETY: the receiver slot, argument and call position follow the prepared method ABI.
    let result = unsafe {
        imre_dyn_positional_method_at(context, callable, operands.cast(), 2, mark, span, scope)
    };
    // SAFETY: the call has returned, so its borrow of this context has ended.
    let ctx = unsafe { &mut *context };
    if ctx.pending.is_some() {
        return NONE;
    }
    let key = ctx.int(i64::from(index));
    let result = ctx.get_item(result, key);
    // SAFETY: the subscript position is null or static.
    finish_at(ctx, result, unsafe { get_span.as_ref() })
}

/// Invoke a call whose positional operands remain rooted in the caller's frame.
///
/// # Safety
/// The generated caller owns the context, supplies a null or static position,
/// and provides a readable array of `count` handles whenever `count` is nonzero.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_positional_call_at(
    ctx: *mut Context,
    callable: Id,
    values: *const Id,
    count: usize,
    mark: usize,
    span: *const [u32; 4],
    scope: Id,
) -> Id {
    // SAFETY: both pointers follow the generated call ABI.
    let (ctx, span) = unsafe { (&mut *ctx, span.as_ref()) };
    scoped_call(ctx, span, |ctx| {
        // SAFETY: the generated caller retains all operands and writes the array before this call.
        let args = copy_ids(unsafe { live_handles(values, count) })?;
        let result = ctx.call_in_scope(scope, callable, args, Vec::new())?;
        ctx.finish_call_operands(mark, result);
        ctx.check_call_return(result)
    })
}

/// Clear the recorded call position and publish the return value or exception.
fn scoped_call(
    ctx: &mut Context,
    span: Option<&[u32; 4]>,
    invoke: impl FnOnce(&mut Context) -> Result<Id>,
) -> Id {
    if ctx.pending.is_some() {
        ctx.heap.set_call_position(None);
        return NONE;
    }
    let result = invoke(ctx);
    ctx.heap.set_call_position(None);
    finish_at(ctx, result, span)
}

/// Record a class annotation only when its statement executes.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_mark_class_annotation(
    ctx: *mut Context,
    scope: Id,
    name: Id,
    offset: u32,
) -> Id {
    // SAFETY: the compiler passes the active context and handles from the current class body.
    let ctx = unsafe { &mut *ctx };
    let result = ctx.record_class_annotation(scope, name, offset);
    finish(ctx, result)
}

/// The compiler emits a native entry when body_line is zero, or generator source otherwise.
#[repr(C)]
union PrelinkedEntry {
    native: Code,
    source: *const c_char,
}

#[repr(C)]
pub(super) struct PrelinkedSite {
    entry: PrelinkedEntry,
    filename: *const c_char,
    first_line: u32,
    body_line: u32,
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_define(
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
        // SAFETY: the compiler tags native functions with line zero and suspended suites
        // with their one-based source line and a static, nul-terminated UTF-8 string.
        let body = unsafe {
            if definition.body_line == 0 {
                value::Body::Native(definition.entry.native)
            } else {
                value::Body::PendingGenerator {
                    source: cstr(definition.entry.source),
                    body_line: definition.body_line,
                }
            }
        };
        let code = ctx.prelinked_code(
            body,
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
    /// Resolve a code object's lexical reads against its enclosing function scopes.
    pub(super) fn enclosing_freevars<'a>(
        &self,
        scope: Id,
        reads: impl Iterator<Item = &'a str>,
    ) -> Option<Vec<String>> {
        let mut freevars = Vec::new();
        for name in reads {
            if self.is_enclosing_freevar(scope, name)? {
                freevars.push(name.to_owned());
            }
        }
        Some(freevars)
    }

    /// A method captures __class__ from its class and other names from the enclosing function.
    pub(super) fn method_freevars<'a>(
        &self,
        enclosing: Option<Id>,
        reads: impl Iterator<Item = &'a str>,
        captures_class: bool,
    ) -> Option<Vec<String>> {
        let mut freevars =
            self.enclosing_freevars(enclosing?, reads.filter(|&name| name != "__class__"))?;
        if captures_class {
            freevars.push("__class__".to_owned());
            freevars.sort();
        }
        Some(freevars)
    }

    /// Determine whether a read resolves to an enclosing function rather than module globals.
    pub(super) fn is_enclosing_freevar(&self, mut scope: Id, name: &str) -> Option<bool> {
        loop {
            let frame = self.scope_data(scope);
            let Some(parent) = frame.parent else {
                return Some(false);
            };
            // A separate exec/eval locals mapping supplies names to the module, never closure cells.
            if frame.exec_locals.is_some() {
                return Some(false);
            }
            if frame.class_body.is_none() {
                if frame.globals.contains(name) {
                    return Some(false);
                }
                if frame.closure.declared.contains(name) || frame.closure.cells.contains_key(name) {
                    return Some(true);
                }
                if !frame.closure.declared_complete {
                    return None;
                }
            }
            scope = parent;
        }
    }

    /// Share an enclosing lexical slot with a function closure or an exposed class frame.
    pub(super) fn capture_free_cell(&mut self, mut cursor: Id, name: &str) -> Id {
        loop {
            let frame = self.scope_data(cursor);
            if let Some(&cell) = frame.closure.cells.get(name) {
                return cell;
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
                frame.closure.cells.insert(name.to_owned(), cell);
                return cell;
            }
            cursor = frame.parent.expect("free variable has enclosing scope");
        }
    }

    pub(super) fn prelinked_code(
        &mut self,
        body: value::Body,
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
        let annotation_class = metadata["annotation_class"] == true;
        let mut read_class = false;
        let reads = metadata["scope_reads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter(|name| {
                if annotation_class && *name == "__class__" {
                    read_class = true;
                    false
                } else {
                    !annotation_class || *name != "__classdict__"
                }
            });
        let lexical = if annotation_class {
            self.scope_data(scope).parent.unwrap_or(scope)
        } else {
            scope
        };
        let freevars = if metadata["scope_complete"] != true {
            None
        } else if let Some(captures_class) = metadata["method_class_cell"].as_bool() {
            self.method_freevars(self.scope_data(scope).parent, reads, captures_class)
        } else if metadata["class_cell"] == true {
            None
        } else {
            self.enclosing_freevars(lexical, reads)
        }
        .map(|mut freevars| {
            if annotation_class {
                freevars.push("__classdict__".to_owned());
                if read_class {
                    freevars.push("__class__".to_owned());
                }
                freevars.sort();
            }
            freevars
        });
        let closure_supported = freevars.is_some();
        let freevars = freevars.unwrap_or_default();
        let flags = metadata["code_flags"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok());
        self.alloc(Value::Code(value::CodeObject {
            parsed_parameters: Default::default(),
            body,
            name,
            qualname,
            doc,
            parameters,
            filename,
            first_line: location.line,
            module_definition,
            coroutine: false,
            flags: flags.unwrap_or_default(),
            flags_complete: flags.is_some(),
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
        let scope = if owner.exec_locals.is_some()
            || ((owner.class_body.is_some() || owner.class_namespace.is_some())
                && !annotation_scope)
        {
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
            if name == "__classdict__" && signature["annotation_class"] == true {
                cells.push(self.class_annotation_dict_cell(scope));
                continue;
            }
            if name == "__class__"
                && (signature["method_class_cell"] == true || signature["annotation_class"] == true)
            {
                cells.push(class_cell.expect("class scope"));
                continue;
            }
            cells.push(self.capture_free_cell(scope, name));
        }
        let scope = if self.code_object(code).closure_supported {
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

/// Drop the finished iteration's temporary handles before asking for the next item.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_comprehension_step(ctx: *mut Context, mark: usize) {
    // SAFETY: the generated caller owns the context and recorded this mark in its active frame.
    unsafe { (&mut *ctx).comprehension_step(mark) }
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
        ctx.finish_binding_release();
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
    if ctx.heap.assignment_target(live, replaced == TRUE)
        && !ctx.can_skip_finalizer_sweep(ctx.heap.temporary_mark())
    {
        ctx.collect_unreachable();
    }
}

/// Move the executing frame to an implicit call site without releasing statement temporaries.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_set_statement_line(ctx: *mut Context, line: u32) {
    // SAFETY: the generated caller owns the context and has entered a statement.
    let ctx = unsafe { &mut *ctx };
    ctx.heap.set_statement_line(line);
    ctx.heap
        .replace_expression(value::ExpressionPosition::Statement);
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
    #[cfg(unix)]
    if let Err(mut error) = ctx.dispatch_signals() {
        ctx.record_exception_frame(&mut error, None);
        ctx.pending = Some(error);
    }
}

impl Context {
    /// Check the old binding without calling a Python mapping's lookup hook.
    pub(super) fn replacing_finalizer(&self, scope: Id, name: &str, value: Id) -> bool {
        if !self.heap.tracks_finalizers() {
            return false;
        }
        let scope = self.binding_scope(scope, name);
        let data = self.scope_data(scope);
        data.exec_locals.is_some()
            || data.exec_globals.is_some()
            || data.class_body_mapping().is_some()
            || self
                .scope_name(scope, name)
                .is_some_and(|old| old != value && self.heap.could_own_finalizer(old, false))
    }

    pub(super) fn heap_and_roots(&mut self) -> (&mut Heap, impl Iterator<Item = Id> + '_) {
        self.threads.reap(&mut self.heap);
        self.reap_thread_operands();
        (
            &mut self.heap,
            self.classes
                .values()
                .chain(self.enum_scopes.values())
                .chain(self.signal_handlers.iter())
                .chain(self.typing.roots())
                .chain(self.builtins.values())
                .chain(self.native_type_dicts.values())
                .chain(self.native_code_objects.values())
                .chain(self.warning_state.iter().flat_map(|state| state.roots()))
                .chain(self.codec_entries.roots())
                .chain(&self.audit_hooks)
                .chain(self.fork_hooks.iter().flatten())
                .chain(&self.exit_callbacks)
                .copied()
                .chain(self.workers.roots().copied())
                .chain(self.threads.roots())
                .chain(self.fault_file)
                .chain(self.contexts.iter().copied())
                .chain(self.buffer_exports.iter().copied())
                .chain(self.pending.iter().flat_map(Error::roots))
                .chain(self.original_builtins)
                .chain(self.source_builtins)
                .chain([
                    self.argv,
                    self.sys_path,
                    self.zip_path_importers,
                    self.modules,
                    self.import_missing,
                    self.bootstrap_globals,
                    self.bootstrap_module,
                    self.bootstrap_file_spec,
                    self.finder_external,
                    self.default_encoding,
                    self.token_missing,
                ]),
        )
    }

    pub(super) fn checkpoint(&mut self, live: &[Id], location: value::Location, caught: Id) {
        self.heap.publish_statement(live, location, caught);
        self.collect_garbage();
    }

    /// Collect unreachable values before a native iterator step grows the heap.
    pub(super) fn collect_streaming_garbage(&mut self, limit: &mut usize) {
        let due = self.heap.prepare_streaming_collection(*limit);
        self.collect_garbage();
        if due {
            *limit = self.heap.streaming_collection_limit();
        }
    }

    /// Run deferred callbacks when a native loop reaches a collection point.
    pub(super) fn collect_garbage(&mut self) {
        if self.heap.gc_schedule.requested() {
            // Root publication is complete here. Retire ordinary expression temporaries
            // before deciding whether live GC allocations exceed the threshold.
            self.heap.gc_schedule.begin_settling();
            self.collect_unreachable();
            self.heap.gc_schedule.end_settling();
            if let Some(generation) = self
                .heap
                .gc_schedule
                .generation_if_due(&self.heap.gc_settings)
            {
                let _ = self.collect_generation(generation);
            }
            return;
        }
        if self.heap.has_ordinary_finalizers()
            && !self.heap.caller_keeps_only_finalizer()
            // A bound stream can keep its underlying finalizer alive without naming it.
            && !self.can_skip_finalizer_sweep(self.heap.temporary_mark())
            && self.heap.has_unbound_finalizer(NONE)
        {
            match self.drain_acyclic_finalizers() {
                Ok(true) => {
                    // Dropping a finalized instance releases its remaining strong edges,
                    // including native resources whose close is visible to the next operand.
                    self.collect_unreachable();
                    return;
                }
                Err(_) => return,
                Ok(false) => {}
            }
        }
        let finalizers = self.heap.take_finalizer_check();
        if self.heap.take_sweep_request()
            || (finalizers && !self.can_skip_finalizer_sweep(self.heap.temporary_mark()))
        {
            let (heap, roots) = self.heap_and_roots();
            if let Some(callbacks) = heap.collect(roots) {
                self.run_weak_callbacks(callbacks);
            }
        }
    }

    /// Flush unreachable file owners before the caller evaluates its next operand.
    pub(super) fn leave_frame(&mut self, result: Id, returning: bool) {
        if self.heap.leave(result, returning)
            && !self.can_skip_finalizer_sweep(self.heap.temporary_mark())
        {
            self.collect_unreachable();
        }
    }

    /// Publish the surviving roots before the next operand can observe a discarded writer.
    pub(super) fn discard_truth(&mut self, mark: usize) {
        self.discard_temporaries(mark, None);
    }

    /// Release discarded operands and root their result before any weak callbacks or finalizers run.
    pub(super) fn discard_temporaries(&mut self, mark: usize, result: Option<Id>) {
        let collect = if (!self.heap.tracks_finalizers() && !self.heap.has_weak_values)
            || self.can_skip_finalizer_sweep(mark)
        {
            self.heap.release_temporaries_since(mark);
            false
        } else {
            self.heap.discard_truth_operands(mark, self.modules)
        };
        if let Some(result) = result {
            self.heap.keep(result);
        }
        if collect {
            self.collect_unreachable();
        }
    }

    /// Release temporary operands while their return value or raised exception stays live.
    pub(super) fn release_with_result(&mut self, mark: usize, result: Result<Id>) -> Result<Id> {
        let collect = self.heap.discard_truth_operands(mark, self.modules);
        match &result {
            Ok(value) => self.heap.keep(*value),
            Err(error) => {
                for root in error.roots() {
                    self.heap.keep(root);
                }
            }
        }
        if collect {
            self.collect_unreachable();
        }
        result
    }

    pub(super) fn finalizer_path_roots(&self) -> [Id; 2] {
        use super::stdlib::datetime::isocalendar::MODULE_REF;
        [
            self.modules,
            self.builtins.get(MODULE_REF).copied().unwrap_or(NONE),
        ]
    }

    /// Skip the sweep only while surviving roots keep every finalizer and weak edge alive.
    fn can_skip_finalizer_sweep(&self, retained_current: usize) -> bool {
        if self.heap.full_collection_active() {
            return false;
        }
        let globals = self.finalizer_path_roots();
        let input = super::stdlib::io::rooted_input_chain(self);
        // Include the three owners retained by sys.__stdin__.
        (self.heap.tracked_finalizer_count() <= super::value::MAX_FRAME_FINALIZERS + 3
            && (!self.heap.has_weak_values
                || self.heap.weak_values_kept_by(globals[0], globals[1]))
            && input
                .as_ref()
                .is_some_and(|chain| self.heap.finalizers_kept_by(chain)))
            || self
                .heap
                .cached_finalizers_kept(globals, input.as_ref(), Some(retained_current))
    }

    /// Keep the returned value and earlier operands while dropping the completed call's temporaries.
    pub(super) fn finish_call_operands(&mut self, mark: usize, result: Id) {
        if !self.heap.tracks_finalizers() {
            return;
        }
        if self.can_skip_finalizer_sweep(mark) {
            self.heap.keep_call_result(mark, result);
        } else if self.heap.discard_operands(mark, result, self.modules) {
            self.collect_unreachable();
        }
    }

    /// Finish an assignment that dropped a possible finalizer owner.
    pub(super) fn finish_binding_release(&mut self) {
        if !self.can_skip_finalizer_sweep(self.heap.temporary_mark()) {
            self.collect_unreachable();
        }
        self.heap.release_unbound_finalizers();
    }

    /// Sweep using the active frames and the context's roots before running callbacks.
    pub(super) fn collect_unreachable(&mut self) {
        if self.drain_acyclic_finalizers().is_err() {
            return;
        }
        let (heap, roots) = self.heap_and_roots();
        if let Some(callbacks) = heap.collect(roots) {
            self.run_weak_callbacks(callbacks);
        }
        let [modules, reference] = self.finalizer_path_roots();
        self.heap.learn_weak_roots(modules, reference);
    }

    /// Resolve weakrefs before callbacks, then recheck the original cyclic isolate.
    pub(super) fn collect_generation(&mut self, generation: u8) -> Result<usize> {
        if self.heap.full_collection_active() {
            return Ok(0);
        }
        self.drain_acyclic_finalizers()?;
        let mark = self.heap.temporary_mark();
        let (heap, roots) = self.heap_and_roots();
        let callbacks = heap.begin_collection(roots, generation)?;
        let check_older_owners = generation < 2
            && (!callbacks.is_empty() || self.heap.full_collection_members().next().is_some());
        let mut released = 0;
        if !callbacks.is_empty() {
            self.run_weak_callbacks(callbacks.iter().copied());
            self.heap.release_temporaries_since(mark);
            let (heap, roots) = self.heap_and_roots();
            let (count, deferred) = heap.release_callback_refs(roots, &callbacks);
            released = count;
            if let Some(deferred) = deferred {
                self.run_weak_callbacks(deferred);
            }
        }
        while let Some(owner) = self.heap.pending_full_finalizer() {
            self.dispatch_finalizer(owner);
        }
        let (heap, roots) = self.heap_and_roots();
        let (count, callbacks) = heap.finish_full_collection(roots);
        if let Some(callbacks) = callbacks {
            self.run_weak_callbacks(callbacks);
        }
        if check_older_owners {
            // Even a resurrected cycle can release its last edge to an older acyclic value.
            self.collect_unreachable();
        }
        self.heap.end_full_collection();
        Ok(count + released)
    }

    /// Invoke pending callbacks after the heap has cleared every dead reference.
    pub(super) fn run_weak_callbacks(&mut self, callbacks: impl IntoIterator<Item = (Id, Id)>) {
        for (callback, reference) in callbacks {
            if let Err(error) = self.call(callback, vec![reference], Vec::new()) {
                // clear_weak_targets retains __callback__ for cyclic GC, but clears it
                // on ordinary release. CPython uses a distinct diagnostic for each.
                let cyclic = self
                    .heap
                    .get(reference)
                    .weak_reference()
                    .is_some_and(|wr| wr.callback != NONE);
                let message = if cyclic {
                    "Exception ignored on calling weakref callback"
                } else {
                    "Exception ignored while calling weakref callback"
                };
                super::unraisable::report(self, error, true, Some(callback), message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::{Rc, Weak};

    #[test]
    fn linked_small_integer_literals_use_permanent_slots() {
        let argv = [c"imre".as_ptr()];
        // SAFETY: the argument and configuration pointers outlive context initialization.
        let mut ctx =
            unsafe { Box::from_raw(imre_dyn_new(1, argv.as_ptr(), c"{\"paths\":[]}".as_ptr())) };
        ctx.heap
            .enter(NONE, NONE, value::FrameOwner::Function(NONE));
        let mark = ctx.heap.temporary_mark();
        for (text, expected) in [("-5", -5), ("0", 0), ("256", 256)] {
            let id = load_linked_literal(&mut ctx, 0, text.as_bytes());
            assert!(matches!(ctx.heap.get(id), Value::Int(n) if n.as_small() == Some(expected)));
            assert_eq!(ctx.heap.temporary_mark(), mark);
        }
        for (text, expected) in [("-6", -6), ("257", 257), ("0xff", 255), ("1_0", 10)] {
            let id = load_linked_literal(&mut ctx, 0, text.as_bytes());
            assert!(matches!(ctx.heap.get(id), Value::Int(n) if n.as_small() == Some(expected)));
        }
    }

    #[test]
    fn dictionary_view_intersection_collects_discarded_values() {
        let argv = [c"imre".as_ptr()];
        // SAFETY: the argument and configuration pointers outlive context initialization.
        let mut ctx =
            unsafe { Box::from_raw(imre_dyn_new(1, argv.as_ptr(), c"{\"paths\":[]}".as_ptr())) };
        ctx.heap
            .enter(NONE, NONE, value::FrameOwner::Function(NONE));
        let dict = ctx.alloc(Value::Dict(Default::default()));
        let keys = ctx.alloc(Value::DictView {
            dict,
            kind: value::DictViewKind::Keys,
            ordered: false,
        });
        let range = crate::dynamic::range::Range::new(0.into(), 16_384.into(), 1.into()).unwrap();
        let source = ctx.alloc(Value::Range(range));

        let mark = ctx.heap.temporary_mark();
        let value: Rc<[Vec<String>]> = Rc::from([]);
        let probe = Rc::downgrade(&value);
        ctx.alloc(Value::AttrGetter(value));
        ctx.heap.release_temporaries_since(mark);

        let selected = ctx.binary("&", keys, source).unwrap();
        assert!(ctx.stored_entries(selected).is_empty());
        assert!(probe.upgrade().is_none());
    }

    #[test]
    fn urlencode_collects_discarded_sequence_values() {
        let argv = [c"imre".as_ptr()];
        // SAFETY: the argument and configuration pointers outlive context initialization.
        let mut ctx =
            unsafe { Box::from_raw(imre_dyn_new(1, argv.as_ptr(), c"{\"paths\":[]}".as_ptr())) };
        ctx.heap
            .enter(NONE, NONE, value::FrameOwner::Function(NONE));
        let range = crate::dynamic::range::Range::new(0.into(), 16_384.into(), 1.into()).unwrap();
        let values = ctx.alloc(Value::Range(range));
        let key = ctx.text("k");
        let pair = ctx.alloc(Value::Tuple(vec![key, values]));
        let query = ctx.alloc(Value::List(vec![pair]));
        let encode = ctx.builtin("urllib.parse.urlencode");

        let mark = ctx.heap.temporary_mark();
        let value: Rc<[Vec<String>]> = Rc::from([]);
        let probe = Rc::downgrade(&value);
        ctx.alloc(Value::AttrGetter(value));
        ctx.heap.release_temporaries_since(mark);

        // The key and decimal values need no escaping.
        let template = ctx.text("{0}");
        let quote_via = ctx.lookup_attr(template, "format").unwrap();
        let safe = ctx.text("");
        let encoded = ctx
            .call(
                encode,
                vec![query, TRUE, safe, NONE, NONE, quote_via],
                Vec::new(),
            )
            .unwrap();
        let encoded = ctx.str_value(encoded).unwrap();
        assert!(encoded.starts_with("k=0&k=1&"));
        assert!(encoded.ends_with("k=16383"));
        assert!(probe.upgrade().is_none());
    }

    #[test]
    fn memory_error_is_ready_before_python_runs() {
        let argv = [c"imre".as_ptr()];
        // SAFETY: the arguments outlive initialization, which returns an owned context.
        let ctx =
            unsafe { Box::from_raw(imre_dyn_new(1, argv.as_ptr(), c"{\"paths\":[]}".as_ptr())) };

        for name in [
            "MemoryError",
            "MemoryError.__new__",
            "BaseException.__init__",
        ] {
            assert!(ctx.builtins.contains_key(name), "{name}");
        }
    }

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
        let code = ctx.prelinked_code(
            value::Body::Native(read),
            outer,
            name,
            signature,
            (name, NONE),
            location.clone(),
        );
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
