//! ABI between LLVM-generated Python functions and Imre-owned objects.

use std::ffi::{CStr, c_char};
use std::slice;
use std::sync::Arc;

use super::source_code::CONDITIONAL;
use super::*;

mod linked_literals;

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

    /// A Python call expression checks for delivery after the callee's native work completes.
    pub(super) fn check_call_return(&mut self, result: Id) -> Result<Id> {
        #[cfg(unix)]
        if crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Relaxed)
            && crate::signals::is_main()
        {
            // A Python handler can delete the last other reference and force collection.
            self.heap.keep(result);
            // SAFETY: the caller retained the result on this thread's active frame.
            unsafe { Self::dispatch_pending_calls(self)? };
        }
        Ok(result)
    }

    /// Instantiate exception classes used by raise and its explicit cause.
    fn instantiate_exception_class(&mut self, value: Id) -> Result<Id> {
        if matches!(self.heap.get(value), Value::Raised(_)) {
            return Ok(value);
        }
        let base = self.builtin("BaseException");
        if !self.direct_subclass(value, base) {
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

    /// Normalize against the class selected by raise, even if constructing the cause
    /// changed the original instance's class.
    fn normalize_raised(&mut self, class: Id, exception: Id) -> Result<Id> {
        let actual = self.exact_type(exception)?;
        let metaclass = self.heap.get(class).instance_class();
        let custom_type = metaclass.is_some_and(|meta| self.builtins.get("type") != Some(&meta));
        let accepted = if custom_type {
            self.python_subclass_check(actual, class)?
        } else {
            self.direct_subclass(actual, class)
        };
        if accepted {
            return Ok(exception);
        }

        let result = self
            .call(class, vec![exception], Vec::new())
            .and_then(|value| {
                if matches!(self.heap.get(value), Value::Raised(_)) {
                    Ok(value)
                } else {
                    Err(Error::new(
                        "TypeError",
                        format!(
                            "calling {} should have returned an instance of BaseException, not {}",
                            self.repr(class),
                            self.type_name(value)
                        ),
                    ))
                }
            });
        result.map_err(|error| {
            let failure = self.raised_instance(error);
            self.heap.keep(failure);
            let args = builtins::call(self, "repr", &[exception], &[])
                .unwrap_or_else(|_| self.text("<unknown>"));
            let prefix = self.text(format!(
                "Normalization failed: type={} args=",
                self.class_name(class)
            ));
            if let Ok(note) = self.concat_strings(prefix, args) {
                let _ = self.add_exception_note(failure, note);
            }
            self.error_from_instance(failure)
        })
    }

    fn error_from_instance(&self, instance: Id) -> Error {
        let Value::Raised(error) = self.heap.get(instance) else {
            unreachable!("expected a normalized exception instance")
        };
        let mut error = error.clone();
        error.tracing().instance = Some(instance);
        error
    }

    /// Propagate an already normalized instance with the active exception as context.
    pub(super) fn raise_normalized(&mut self, instance: Id) -> Result<Id> {
        let mut error = self.error_from_instance(instance);
        self.chain_raised_instance(&mut error);
        Err(error)
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

    /// Resolve positional names only after isinstance succeeds, then fetch all
    /// fields in source order before testing any nested patterns.
    fn match_class(&mut self, subject: Id, class: Id, names: Id) -> Result<Id> {
        if !self.class_pattern_matches(subject, class)? {
            return Ok(NONE);
        }
        let Value::List(names) = self.heap.get(names) else {
            unreachable!("compiled class pattern arguments")
        };
        let mut names = names.clone();
        let positional = names.iter().take_while(|&&name| name == NONE).count();
        let mut fields = Vec::with_capacity(names.len());
        let mut seen = Vec::new();
        let mut first = 0;
        let args_mark = self.heap.temporary_mark();
        if positional > 0 {
            let match_args = self.optional_attr(class, "__match_args__")?;
            let match_self = match_args.is_none() && self.class_matches_self(class)?;
            let elements = match match_args {
                Some(tuple) => match self.heap.get(tuple) {
                    Value::Tuple(items) => items.as_slice(),
                    _ => {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "{}.__match_args__ must be a tuple (got {})",
                                self.class_name(class),
                                self.type_name(tuple)
                            ),
                        ));
                    }
                },
                None => &[],
            };
            let allowed = if match_self { 1 } else { elements.len() };
            if positional > allowed {
                let suffix = if allowed == 1 { "" } else { "s" };
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{}() accepts {allowed} positional sub-pattern{suffix} ({positional} given)",
                        self.class_name(class)
                    ),
                ));
            }
            if match_self {
                self.heap.keep(subject);
                fields.push(subject);
                first = 1;
            } else {
                names[..positional].copy_from_slice(&elements[..positional]);
            }
        }
        for (index, &name) in names.iter().enumerate().skip(first) {
            if index < positional
                && !matches!(self.heap.get(name), Value::Str(_) | Value::SurrogateStr(_))
            {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__match_args__ elements must be strings (got {})",
                        self.type_name(name)
                    ),
                ));
            }
            if positional > 0 {
                if seen.iter().any(|&other| self.equal(other, name)) {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "{}() got multiple sub-patterns for attribute {}",
                            self.class_name(class),
                            self.repr(name)
                        ),
                    ));
                }
                seen.push(name);
            }
            let attr = super::attribute_name::AttributeName::Python {
                key: name,
                text: None,
            };
            match self.lookup_attribute(subject, attr, false) {
                Ok(field) => {
                    self.heap.keep(field);
                    fields.push(field);
                }
                Err(error) if self.is_attribute_error(&error) => return Ok(NONE),
                Err(error) => return Err(error),
            }
            if index + 1 == positional {
                // MATCH_CLASS drops the descriptor's tuple after its positional reads.
                // Retain the extracted values and names across this observable boundary.
                self.discard_temporaries(args_mark, fields.iter().chain(&seen).copied());
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
            "bind_doctest_module" => self.bind_doctest_module(a, b),
            "displayhook" => {
                self.heap.keep(a);
                let hook = self
                    .sys_binding("displayhook")?
                    .ok_or_else(|| Error::new("RuntimeError", "lost sys.displayhook"))?;
                self.heap.keep(hook);
                self.call(hook, vec![a], Vec::new())?;
                Ok(NONE)
            }
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
                    definition: typing::AliasDefinition::Lazy(typing::LazyEvaluation::new(b)),
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
            "build_template" => self.template_from_literal(a, b),
            "linked_string_tuple" => {
                let items = self.items(a)?;
                self.linked_constant(|ctx| {
                    ctx.heap
                        .linked_string_constant(b, |heap| heap.packed_tuple(items, true))
                })
            }
            "linked_string" => self.heap.linked_string_constant(b, |_| a),
            "match_class" => self.match_class(a, b, c),
            "match_mapping" => self.match_mapping(a, b),
            "match_mapping_keys" => self.match_mapping_keys(a, b),
            "match_sequence" => Ok(builtins::boolean(
                self.pattern_flags(a)
                    .has(super::collection_pattern::PatternFlags::SEQUENCE),
            )),
            "match_sequence_length" => builtins::call(self, "len", &[a], &[]),
            "builtin" => {
                let name = self.copy_name(a)?;
                Ok(self.builtin(&name))
            }
            "annotation_scope" => Ok(self
                .scope_data(a)
                .parent
                .expect("annotation function scope")),
            "lookup_annotation_global"
            | "lookup_class_annotation"
            | "lookup_class_annotation_global" => {
                let name = self.copy_name(b)?;
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
                let name = self.copy_name(b)?;
                if op == "lookup_class_local" {
                    return self.lookup_class_local(a, &name);
                }
                self.lookup(a, &name)
            }
            "assign" | "assign_target" | "assign_named_target" => {
                let name = self.copy_name(b)?;
                let scope = if op == "assign_named_target" {
                    self.named_expression_scope(a, &name)
                } else {
                    a
                };
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
                let name = self.copy_name(b)?;
                let released = self.replacing_finalizer(a, &name, NONE);
                let scope = self.binding_scope(a, &name);
                let removed = match self.remove_binding(scope, &name, b) {
                    Err(error)
                        if op == "delname"
                            && self.scope_data(a).parent.is_none()
                            && !self.scope_data(a).globals.contains(name.as_str()) =>
                    {
                        // DELETE_NAME replaces the mapping error after its callback has unwound.
                        self.raised_instance(error);
                        false
                    }
                    result => result?,
                };
                if removed {
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
                let name = self.copy_name(b)?;
                self.module(&name)
            }
            "fromlist" => self.prepare_fromlist(a, b),
            "from" => self.import_from(a, b),
            "import_star" => self.import_star(a, b),
            "lookup_enter" => {
                self.context_method(a, if b == TRUE { "__aenter__" } else { "__enter__" })
            }
            "lookup_exit" => {
                self.context_method(a, if b == TRUE { "__aexit__" } else { "__exit__" })
            }
            "await_iter" => {
                let method = match b {
                    0 => None,
                    1 => Some("__aenter__"),
                    2 => Some("__aexit__"),
                    _ => return Err(Error::new("SystemError", "invalid await context")),
                };
                self.await_iterator(a, method)
            }
            "attr" | "method" => {
                let name = self.copy_name(b)?;
                if op == "method" {
                    self.lookup_method(a, &name)
                } else {
                    self.lookup_attribute_operand(
                        a,
                        super::attribute_name::AttributeName::Text(&name),
                        true,
                        Some(b),
                    )
                }
            }
            "setattr" => self.set_attr(a, &self.copy_name(b)?, c),
            "delattr" => {
                let name = self.copy_name(b)?;
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
                    let mut items = Vec::new();
                    self.expand_positional_items(&mut items, a, Some(b))?;
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
                if matches!(self.heap.get(a), Value::List(_)) {
                    let mut items = Vec::new();
                    self.expand_positional_items(&mut items, b, None)?;
                    let Value::List(list) = self.heap.get_mut(a) else {
                        unreachable!()
                    };
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
                } else if matches!(self.heap.get(a), Value::Set { frozen: false, .. }) {
                    builtins::update_set(self, a, b)?;
                } else {
                    for item in self.items(b)? {
                        self.op("append", a, item, NONE)?;
                    }
                }
                Ok(a)
            }
            "update" => self.merge_mapping(a, b, super::calls::MappingOperation::Unpack),
            "keywords" | "class_keywords" => self.merge_mapping(
                a,
                b,
                super::calls::MappingOperation::Keywords((op == "keywords").then_some(c)),
            ),
            "unpack" | "unpack_read" => {
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
                self.unpack_sequence(a, count, star, op == "unpack_read")
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
            "type_parameter_evaluator" => super::typing::set_parameter_evaluator(self, a, b, c),
            "class_parameter_scope" | "function_parameter_scope" => {
                super::typing::parameter_scope(self, a, b, op == "class_parameter_scope")
            }
            "class_parameter_frame" => super::typing::class_parameter_frame(self, a, b),
            "class_annotation_environment" => Ok(self.class_annotation_environment(a)),
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
                if !self.scope_data(b).in_class_body {
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
            "mark_module_annotation" => {
                self.mark_module_annotation(a, b)?;
                Ok(NONE)
            }
            "mark_class_annotation" => self.record_class_annotation(a, b, c),
            "setup_module_string_annotations" | "setup_module_annotations" => {
                self.setup_module_annotations(a, op == "setup_module_string_annotations")?;
                Ok(NONE)
            }
            "annotate" => {
                if self.scope_data(a).in_class_body {
                    let mapping = self.lookup_class_local(a, "__annotations__")?;
                    self.put_item(mapping, b, c)?;
                } else {
                    let mapping = self.module_annotation_name(a, "__annotations__")?;
                    self.put_item(mapping, b, c)?;
                }
                Ok(NONE)
            }
            "generator" => self.generator(a, b),
            "iter" => self.loop_iterator(a),
            "get_iter" => self.iterator(a),
            "loop_cursor" => Ok(self.loop_cursor(a)),
            "async_iter" => self.async_for_iter(a),
            "async_next_iter" => self.async_for_next(a),
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
            "pos"
                if matches!(
                    self.heap.get(a),
                    Value::Int(_) | Value::Float(_) | Value::Complex(_)
                ) =>
            {
                Ok(a)
            }
            "neg" if let Value::Float(f) = self.heap.get(a) => Ok(self.alloc(Value::Float(-f))),
            "neg" if let Value::Complex(z) = self.heap.get(a) => {
                Ok(self.alloc(Value::Complex(super::complex::Complex {
                    real: -z.real,
                    imag: -z.imag,
                })))
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
                    .heap
                    .execution
                    .pending
                    .take()
                    .ok_or_else(|| Error::new("RuntimeError", "no active exception"))?;
                self.heap.release_unbound_finalizers();
                Ok(self.raised_instance(error))
            }
            "restore_caught" => {
                self.heap.restore_caught(a);
                Ok(NONE)
            }
            "matches" => {
                let base = self.builtin("BaseException");
                let classes = self.tuple_elements(b).unwrap_or(std::slice::from_ref(&b));
                self.check_exception_classes(classes, base)?;
                let matched = matches!(self.heap.get(a), Value::Raised(error)
                    if classes.iter().any(|&class| self.raised_error_matches(error, class)));
                Ok(flag(matched))
            }
            "exit_context" | "async_exit_context" => {
                let active = if b == NONE {
                    self.heap.caught_exception(c)
                } else {
                    b
                };
                let prior = self.heap.replace_active(active);
                let result = self.call_context_exit(a, b);
                // The exceptional exit's truth test still runs inside the handler.
                let result = if b == NONE || op == "async_exit_context" {
                    result
                } else {
                    result.and_then(|value| self.truth(value).map(flag))
                };
                self.heap.replace_active(prior);
                result
            }
            "except_star_split" => {
                let (matched, rest) = super::exception_group::split_star(self, a, b)?;
                Ok(self.alloc(Value::Tuple(vec![matched, rest])))
            }
            "except_star_merge" => {
                let Value::List(raised) = self.heap.get(b) else {
                    return Err(Error::new("SystemError", "expected except* arm outcomes"));
                };
                let raised = super::copy_ids(raised)?;
                super::exception_group::merge_star(self, a, &raised)
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
                if !matches!(self.heap.get(exception), Value::Raised(_)) {
                    return Err(Error::new(
                        "TypeError",
                        "exceptions must derive from BaseException",
                    ));
                }
                if op == "reraise" {
                    return Err(self.error_from_instance(exception));
                }
                let class = if a == exception {
                    self.exact_type(exception)?
                } else {
                    a
                };
                // The cause constructor can mutate __class__ and collect its old value.
                self.heap.keep(class);
                if c == TRUE {
                    let cause = self.instantiate_exception_class(b)?;
                    if cause != NONE && !matches!(self.heap.get(cause), Value::Raised(_)) {
                        return Err(Error::new(
                            "TypeError",
                            "exception causes must derive from BaseException",
                        ));
                    }
                    self.set_exception_cause(exception, cause)?;
                }
                let normalized = self.normalize_raised(class, exception)?;
                self.raise_normalized(normalized)
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
    unsafe { std::str::from_utf8_unchecked(CStr::from_ptr(ptr).to_bytes()) }
}

/// Recover the native filename constant emitted from OsStr::as_encoded_bytes.
unsafe fn native_path<'a>(ptr: *const c_char) -> &'a Path {
    // SAFETY: the compiler emits a nul-terminated native path in the executable's target encoding.
    unsafe {
        Path::new(std::ffi::OsStr::from_encoded_bytes_unchecked(
            CStr::from_ptr(ptr).to_bytes(),
        ))
    }
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
    crate::prepare_stdio(
        ignore_environment,
        config["utf8_mode"].as_bool(),
        config["unbuffered"].as_bool() == Some(true),
    );
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

pub(super) fn new_context<S: Into<crate::Str>>(
    config: serde_json::Value,
    args: impl IntoIterator<Item = S>,
) -> Context {
    let heap = Heap::new();
    if let Some(depth @ 1..=65535) = config["tracemalloc"].as_u64() {
        heap.allocation_traces.start_at_startup(depth as usize);
    }
    new_context_with_heap(Arc::new(config), args, heap, None)
}

pub(super) fn new_context_with_heap<S: Into<crate::Str>>(
    config: Arc<serde_json::Value>,
    args: impl IntoIterator<Item = S>,
    heap: Heap,
    worker_interpreter: Option<InterpreterState>,
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
        threads: lowlevel_threads::Workers {
            private_context: worker_interpreter.is_some(),
            ..Default::default()
        },
        interpreter: worker_interpreter.unwrap_or_default(),
        workers: Default::default(),
        sendfile_writes: Vec::new(),
        config,
        executable,
        helper_executable,
        argv: NONE,
        startup_path0: None,
        saved_entry_root: None,
        default_encoding: NONE,
        displaced_sni_callbacks: Default::default(),
        audit_hooks: Vec::new(),
        fork_hooks: Default::default(),
        fault_file: None,
        fault_user_files: HashMap::new(),
        exit_callbacks: Vec::new(),
        contexts: Vec::new(),
        buffer_exports: Vec::new(),
        repr_ancestors: Vec::new(),
        repr_spare_stack: false,
        // Measure the execution thread before entering the native Python evaluator.
        repr_sequence_words: super::operators::remaining_repr_words(),
        suspended_typing_frames: 0,
        token_missing: NONE,
        native_filename_labels: HashMap::new(),
        original_builtins: None,
        source_builtins: None,
        zip_directories: HashMap::new(),
        zip_path_importers: NONE,
        codec_entries: Default::default(),
        warning_state: None,
        frozen_override: 0,
        pending_builtin_names_scope: None,
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
        reporting_wakeup_errors: false,
        typing: super::typing::Registry::default(),
        builtins: HashMap::new(),
        scope_name_slots: Default::default(),
        native_type_dicts: HashMap::new(),
        sources: HashMap::new(),
        call_depth: 0,
        deque_comparison_repeats: 0,
        slot_depth: 0,
    };
    ctx.modules = ctx.dict(Vec::new());
    let object = ctx.builtin("object");
    ctx.import_missing = ctx.instance(object, None);
    let args = args.into_iter().map(|s| ctx.text(s)).collect();
    ctx.argv = ctx.alloc(Value::List(args));
    ctx.default_encoding = ctx.heap.code_name(crate::sys::default_encoding().as_str());
    let current = ctx.alloc(Value::Context(Default::default()));
    ctx.contexts.push(current);
    ctx.token_missing = ctx.alloc(Value::TokenMissing);
    // Resolving the handler after an allocation failure must not grow the builtin cache.
    ctx.builtin("MemoryError");
    // These are already-present runtime types, including the types of singletons,
    // code, functions and iterators. Publish them before user-defined subclasses.
    for name in builtins::core_types() {
        ctx.builtin(name);
    }
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
    // SAFETY: compiled entries supply static module names, source and executable code.
    unsafe { register_compiled(ctx, name, file, text, len, ModuleInitializer::Native(code)) }
}

/// Attach a checked interactive input immediately after registering its owning module.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_module_single(
    ctx: *mut Context,
    owner: *const c_char,
    filename: *const c_char,
    source: *const u8,
    len: u64,
    flags: u32,
    code: Code,
) {
    // SAFETY: the linked entry supplies static filenames, native code and exactly len source bytes.
    let (ctx, owner, filename, source) = unsafe {
        (
            &mut *ctx,
            cstr(owner),
            cstr(filename),
            std::slice::from_raw_parts(source, len as usize),
        )
    };
    let module = ctx
        .sources
        .get_mut(owner)
        .and_then(|sources| sources.last_mut())
        .expect("registered prompt owner");
    Rc::make_mut(module.single_inputs.get_or_insert_with(Default::default)).push(SingleInput {
        filename,
        source,
        flags,
        code,
    });
}

/// Accept older physical file registrations that do not carry compiler flags.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_file_source(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    digest: *const u8,
    len: u64,
    code: Code,
) {
    // Older file registrations supplied no fingerprint.
    let digest = if len == 32 && !digest.is_null() {
        // SAFETY: the compiler emits a static constant containing all 32 SHA-256 bytes.
        Some(unsafe { &*digest.cast::<[u8; 32]>() })
    } else {
        None
    };
    // SAFETY: the compiler supplies static names, filename and initializer.
    unsafe {
        register_compiled(
            ctx,
            name,
            file,
            std::ptr::null(),
            0,
            ModuleInitializer::File {
                entry: code,
                digest,
                flags: None,
                declared_open_path: None,
            },
        )
    }
}

/// Register a physical file's checked bytes, future flags and compiled initializer.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_file_code(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    digest: *const [u8; 32],
    flags: u32,
    code: Code,
) {
    // SAFETY: the compiler supplies the static operands of a physical file registration.
    unsafe { register_file_code(ctx, name, file, digest, flags, code, None) }
}

/// Register the physical file selected for an explicitly included module.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_declared_file_code_at(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    digest: *const [u8; 32],
    flags: u32,
    code: Code,
    file_to_open: *const c_char,
) {
    // SAFETY: the compiler supplies the static operands of the selected file registration.
    unsafe {
        register_file_code(
            ctx,
            name,
            file,
            digest,
            flags,
            code,
            Some(native_path(file_to_open)),
        )
    }
}

unsafe fn register_file_code(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    digest: *const [u8; 32],
    flags: u32,
    entry: Code,
    declared_open_path: Option<&'static Path>,
) {
    // SAFETY: the compiler supplies a static digest, names and module entry point.
    unsafe {
        register_compiled(
            ctx,
            name,
            file,
            std::ptr::null(),
            0,
            ModuleInitializer::File {
                entry,
                digest: Some(&*digest),
                flags: Some(flags),
                declared_open_path,
            },
        );
    }
}

/// Register the compiled body for a ZIP member without adding a directory finder.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_zip_source(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    digest: *const [u8; 32],
    flags: u32,
    code: Code,
) {
    // SAFETY: native registration supplies static names, digest and entry point.
    unsafe {
        register_compiled(
            ctx,
            name,
            file,
            std::ptr::null(),
            0,
            ModuleInitializer::Zip(code, &*digest, flags),
        )
    }
}

/// Register compiled Python that runs after a built-in module installs its native bindings.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_builtin_source(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    text: *const u8,
    len: u64,
    code: Code,
) {
    // SAFETY: the compiler supplies the same static storage as for a file initializer.
    unsafe { register_compiled(ctx, name, file, text, len, ModuleInitializer::Builtin(code)) }
}

unsafe fn register_compiled(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    text: *const u8,
    len: u64,
    initializer: ModuleInitializer,
) {
    // SAFETY: module names and filenames are static compiler-emitted strings.
    let (ctx, name, file) = unsafe { (&mut *ctx, cstr(name), native_path(file)) };
    // SAFETY: non-null text points to a static source constant with exactly len bytes.
    let bundled_text =
        (!text.is_null()).then(|| unsafe { std::slice::from_raw_parts(text, len as usize) });
    register_source(ctx, name, file, initializer, bundled_text);
}

/// Register a discovery-time error and preserve the source for loader inspection.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_register_module_error(
    ctx: *mut Context,
    name: *const c_char,
    file: *const c_char,
    text: *const u8,
    len: u64,
    message: *const c_char,
    position: *const [u32; 4],
    details: u8,
    archive_digest: *const [u8; 32],
) {
    // SAFETY: the compiler emits all strings and the position as static constants.
    unsafe {
        let position = position.as_ref();
        let parser_line = (details == 2).then(|| {
            let source = std::str::from_utf8(std::slice::from_raw_parts(text, len as usize))
                .expect("compiler emitted UTF-8");
            source
                .split_inclusive('\n')
                .nth(position.expect("parser error position")[0] as usize - 1)
                .unwrap_or("")
        });
        register_compiled(
            ctx,
            name,
            file,
            text,
            len,
            ModuleInitializer::Invalid {
                message: cstr(message),
                position,
                in_args: details != 0,
                parser_line,
                archive_digest: archive_digest.as_ref(),
            },
        );
    }
}

/// Bind an emitted expression to its actual annotation string. Equal runtime
/// text has no association with it. The heap forgets the binding on collection.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_bind_expression(ctx: *mut Context, text: Id, entry: Code) {
    // SAFETY: compiled callers pass the active interpreter and a linked native entry.
    let ctx = unsafe { &mut *ctx };
    ctx.heap
        .linked_sources
        .insert(text, LinkedSource::Expression(entry));
}

/// Select the prelinked module for the actual builtin and the exact source at this call.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_exec_operand(
    ctx: *mut Context,
    callable: Id,
    text: Id,
    entry: Code,
    flags: u32,
    expected: *const c_char,
) -> Id {
    // SAFETY: compiled callers supply an active context and static NUL-terminated UTF-8 source.
    let (ctx, expected) = unsafe { (&mut *ctx, cstr(expected)) };
    if !matches!(ctx.heap.get(callable), Value::Native(name) if name == "exec")
        || !matches!(ctx.heap.get(text), Value::Str(value) if value.as_str() == expected)
    {
        return text;
    }
    if let Err(error) = ctx.heap.reserve_temporaries(3) {
        return finish(ctx, Err(error));
    }
    let filename = ctx.text("<string>");
    ctx.native_module_code(entry, filename, Some(flags))
}

fn register_source(
    ctx: &mut Context,
    name: &str,
    file: &Path,
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
            single_inputs: None,
        });
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_finish(ctx: *mut Context) -> i32 {
    // SAFETY: main transfers its unique allocation from imre_dyn_new exactly once.
    let mut ctx = unsafe { Box::from_raw(ctx) };
    let _publication =
        crate::fault_handler::FramePublication::enter(ctx.interpreter.finalizing.clone());
    let mark = ctx.heap.temporary_mark();
    let mut failed_site = false;
    // With unfrozen modules, importing site can execute a filesystem replacement for stat.
    // Preserve the existing hook-triggered startup in that mode.
    let startup_site = ctx.config["executable"].is_string()
        && ctx.config["no_site"] != true
        && (ctx.config["frozen_modules"] != false || ctx.config["sitecustomize"] == true);
    let frame = ctx.startup_sys().and_then(|launched| {
        if ctx.config["path0"] == true && !startup_site {
            ctx.insert_startup_path0(launched)?;
        }
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
        if startup_site {
            let mark = ctx.heap.temporary_mark();
            ctx.initialize_startup_site()
                .inspect_err(|_| failed_site = true)?;
            ctx.heap.release_temporaries_since(mark);
            if ctx.config["path0"] == true {
                ctx.insert_startup_path0(launched)?;
            }
        }
        let name = ctx.text("__main__");
        ctx.prepare_main_module()?
            .ok_or_else(|| Context::missing_module("__main__", name))
    });
    let main = match frame {
        // SAFETY: finish owns the boxed context. The loader's borrow has ended and its
        // rooted frame remains active until the native script returns.
        Ok(frame) => unsafe { frame.run(&raw mut *ctx) },
        Err(error) => Err(error),
    };
    let result = main.and_then(|main| {
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
        stdlib::io::flush_standard_outputs(&mut ctx);
        crate::fault_handler::unregister_all();
        #[cfg(unix)]
        crate::signals::finalize();
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
    #[cfg(unix)]
    if crate::signals::is_main() {
        crate::signals::disable_wakeup();
    }
    // Exit callbacks run before Python enters its finalization phase.
    ctx.interpreter
        .finalizing
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
    let status = if stdlib::io::flush_standard_outputs(&mut ctx) {
        status
    } else {
        120
    };
    stdlib::io::prepare_writer_shutdown(&ctx);
    crate::fault_handler::unregister_all();
    crate::fault_handler::disable();
    #[cfg(unix)]
    crate::signals::finalize();
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

pub(super) fn finish(ctx: &mut Context, result: Result<Id>) -> Id {
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
            ctx.heap.execution.pending = Some(error);
            NONE
        }
    }
}

/// Snapshot a fixed pair before either read-only unpacking target is assigned.
///
/// # Safety
/// Generated code supplies the active context and two writable handle slots.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_unpack_pair(
    ctx: *mut Context,
    value: Id,
    items: *mut [Id; 2],
) -> Id {
    // SAFETY: the emitter allocates both slots in the active native frame.
    let (ctx, items) = unsafe { (&mut *ctx, &mut *items) };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = snapshot_pair(ctx, value, items);
    finish(ctx, result)
}

/// Snapshot two items and return the handle that must keep them alive during assignment.
fn snapshot_pair(ctx: &mut Context, value: Id, items: &mut [Id; 2]) -> Result<Id> {
    match ctx.heap.get(value) {
        Value::Tuple(values) if values.len() == 2 => {
            items.copy_from_slice(values);
            Ok(value)
        }
        Value::List(values) if values.len() == 2 => {
            items.copy_from_slice(values);
            // Assigning the first target can mutate the list. Keep both original
            // items in the current frame; finish roots the second handle.
            ctx.heap.reserve_temporaries(2).map(|()| {
                ctx.heap.keep(items[0]);
                items[1]
            })
        }
        _ => ctx.unpack_items(value, 2, None).map(|values| {
            // The iterator path roots each handle as it consumes it.
            items.copy_from_slice(&values);
            NONE
        }),
    }
}

/// Pull directly from an acquired builtin sequence or range iterator.
/// Return 1 for an item, 0 for exhaustion or error, and -1 for other iterators.
///
/// # Safety
/// Generated code supplies a live cursor and one writable handle slot.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_list_cursor_pull(
    ctx: *mut Context,
    iter: Id,
    item: *mut Id,
    has_else: bool,
) -> i32 {
    // SAFETY: the caller keeps the context and cursor live throughout the pull.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return 0;
    }
    let next = match ctx.heap.get(iter) {
        Value::Iterator { .. } => ctx.next_builtin_sequence_item(iter),
        Value::RangeIterator(_) => ctx.next_result(iter),
        _ => return -1,
    };
    let value = match next {
        Ok(value) => value,
        Err(error) => {
            finish(ctx, Err(error));
            return 0;
        }
    };
    if let Some(value) = value {
        // The loop body can remove this item and collect its former owner.
        let value = finish(ctx, Ok(value));
        if ctx.heap.execution.pending.is_some() {
            return 0;
        }
        // SAFETY: generated code supplies the slot for the rooted item.
        unsafe { item.write(value) };
        1
    } else {
        if has_else {
            ctx.heap.release_unbound_finalizers();
        }
        0
    }
}

/// Query the acquired cursor without advancing it or invoking Python.
///
/// # Safety
/// Generated code supplies the active context and a live loop cursor.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_plain_items_cursor(ctx: *const Context, iter: Id) -> bool {
    // SAFETY: the generated caller keeps the context and cursor live for this query.
    plain_items_cursor(unsafe { &*ctx }, iter).is_some()
}

fn plain_items_cursor(ctx: &Context, iter: Id) -> Option<(Id, DictIterator)> {
    let Value::LoopIterator {
        source,
        peeked: None,
    } = *ctx.heap.get(iter)
    else {
        return None;
    };
    let Value::DictIterator(cursor) = *ctx.heap.get(source) else {
        return None;
    };
    cursor.is_plain_items().then_some((source, cursor))
}

/// Advance a compiled loop whose target unpacks exactly two items.
///
/// # Safety
/// Generated code supplies a live context and space for two handles.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_next_pair(
    ctx: *mut Context,
    iter: Id,
    items: *mut [Id; 2],
    has_else: bool,
) -> Id {
    // SAFETY: generated code owns these scratch slots in the active frame.
    let (ctx, items) = unsafe { (&mut *ctx, &mut *items) };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = next_loop_pair(ctx, iter, items, has_else);
    finish(ctx, result)
}

/// Read dictionary entries directly, keeping both values alive across target assignment.
fn next_loop_pair(ctx: &mut Context, iter: Id, items: &mut [Id; 2], has_else: bool) -> Result<Id> {
    let more = if let Some((source, cursor)) = plain_items_cursor(ctx, iter) {
        if let Some(pair) = cursor.next_entry_pair(ctx, source)? {
            ctx.heap.reserve_temporaries(2)?;
            for value in pair {
                ctx.heap.keep(value);
            }
            *items = pair;
            true
        } else {
            false
        }
    } else if ctx.iterator_has_item(iter)? {
        let value = ctx
            .next_item(iter)?
            .ok_or_else(|| Error::new("StopIteration", ""))?;
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(value);
        let owner = snapshot_pair(ctx, value, items)?;
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(owner);
        true
    } else {
        false
    };
    if !more && has_else {
        ctx.heap.release_unbound_finalizers();
    }
    Ok(flag(more))
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
    // SAFETY: the legacy entry supplies the same permanent handles as a byte count.
    unsafe { imre_dyn_build_sequence(ctx, data.cast(), len / 2, true, tuple, std::ptr::null()) }
}

/// Build a display from evaluated handles or from linked immutable handle data.
///
/// # Safety
/// The caller supplies a live context and `count` rooted native handles, or little-endian
/// u16 permanent handles when `packed` is set. A nonnull slot identifies a constant tuple.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_build_sequence(
    ctx: *mut Context,
    data: *const Id,
    count: usize,
    packed: bool,
    tuple: bool,
    slot: *const u8,
) -> Id {
    // SAFETY: generated code supplies its active context.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let items = || -> Result<Vec<Id>> {
        let mut items = Vec::new();
        items
            .try_reserve_exact(count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        if packed {
            // SAFETY: little-endian pairs in module data need not be aligned.
            let pairs = unsafe { slice::from_raw_parts(data.cast::<[u8; 2]>(), count) };
            items.extend(pairs.iter().map(|&pair| Id::from(u16::from_le_bytes(pair))));
        } else {
            // SAFETY: a zero-length display may pass null; the frame roots all other handles.
            items.extend_from_slice(unsafe { live_handles(data, count) });
        }
        Ok(items)
    };
    let result = if !slot.is_null() {
        ctx.linked_constant(|ctx| {
            ctx.heap
                .cached_constant(super::value::ConstantSlot::Linked(slot.addr()), |heap| {
                    Ok(heap.packed_tuple(items()?, true))
                })
        })
    } else if tuple && packed {
        ctx.linked_constant(|ctx| Ok(ctx.heap.packed_tuple(items()?, true)))
    } else {
        items().map(|items| {
            if tuple {
                ctx.heap.packed_tuple(items, false)
            } else {
                ctx.alloc(Value::List(items))
            }
        })
    };
    finish(ctx, result)
}

/// Transfer the evaluated keys to a fresh set in operand order, including the release of
/// duplicates and, following an insertion error, the remaining keys before the partial set.
///
/// # Safety
/// The compiler supplies a live context, rooted handles, and the mark before their evaluation.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_build_set(
    ctx: *mut Context,
    data: *const Id,
    count: usize,
    mark: usize,
) -> Id {
    // SAFETY: the caller keeps this frame and its handle array live across hash/equality callbacks.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    // SAFETY: the compiler roots each key and keeps its native operand array live for this call.
    let keys = unsafe { live_handles(data, count) };
    let result = (|| {
        ctx.heap.reserve_temporaries(1 + count)?;
        let set = ctx.op("set", NONE, NONE, NONE)?;
        ctx.discard_temporaries(mark, std::iter::once(set).chain(keys.iter().rev().copied()));
        let mut result = Ok(set);
        for (index, &key) in keys.iter().enumerate() {
            if result.is_ok() {
                result = builtins::set_add(ctx, set, key).map(|()| set);
            }
            let key_mark = mark + count - index;
            match &result {
                Ok(_) => ctx.discard_truth(key_mark),
                Err(error) => ctx.discard_temporaries(key_mark, error.roots()),
            }
        }
        ctx.release_with_result(mark, result)
    })();
    finish(ctx, result)
}

/// Intern an already evaluated immutable value in its linked source unit.
///
/// # Safety
/// The context and value must be live and the compiler slot must have a stable, unique address.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_linked_value(
    ctx: *mut Context,
    slot: *const u8,
    value: Id,
) -> Id {
    // SAFETY: generated code keeps the value rooted in its live context.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = ctx
        .heap
        .cached_constant(super::value::ConstantSlot::Linked(slot.addr()), |_| {
            Ok(value)
        });
    finish(ctx, result)
}

/// Insert one streamed display operand and transfer its roots to the receiving list or set.
///
/// # Safety
/// The compiler passes its live context, a mutable list or set, its operand and their mark.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_display_item(
    ctx: *mut Context,
    list: Id,
    item: Id,
    mark: usize,
    extend: bool,
) -> Id {
    let op = if extend { c"extend" } else { c"append" };
    // SAFETY: this is the ordinary checked operation on the same rooted operands.
    let result = unsafe { imre_dyn_op_at(ctx, op.as_ptr(), list, item, NONE, std::ptr::null()) };
    if result != NONE {
        // SAFETY: both operations return their target container on success.
        unsafe { &mut *ctx }.discard_temporaries(mark, [result]);
    }
    result
}

/// Pop a failed display's operands in stack order while their Python exception stays active.
///
/// # Safety
/// The context and mark must belong to the active registered frame.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_unwind_display(ctx: *mut Context, mark: usize) {
    // SAFETY: the generated error edge is in the frame which established the mark.
    unsafe { &mut *ctx }.unwind_operands(mark);
}

/// Borrow a tagged integer from an exact Python int. Zero indicates another type.
///
/// # Safety
/// Generated code keeps the object rooted until its last use of the borrowed word.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_exact_int_word(ctx: *const Context, object: Id) -> usize {
    // SAFETY: the caller supplies its active context and a live handle.
    match unsafe { &*ctx }.heap.get(object) {
        Value::Int(integer) => integer.0,
        _ => 0,
    }
}

/// Compare against an integer whose tagged word the caller already has.
///
/// # Safety
/// The frame supplies a live context, two rooted operands, a comparison opcode,
/// and the original right operand's tagged word.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_compare_integer_rhs(
    ctx: *mut Context,
    opcode: *const c_char,
    left: Id,
    right: Id,
    word: i64,
) -> Id {
    // SAFETY: the generated frame owns the context and supplies a static opcode.
    let context = unsafe { &*ctx };
    if context.heap.execution.pending.is_some() {
        return NONE;
    }
    if let Value::Int(x) = context.heap.get(left)
        && let Some(x) = x.as_small()
    {
        let y = word >> 1;
        // SAFETY: opcode names one of the six checked comparison operators.
        return flag(match unsafe { cstr(opcode) } {
            "==" => x == y,
            "!=" => x != y,
            "<" => x < y,
            "<=" => x <= y,
            ">" => x > y,
            ">=" => x >= y,
            _ => unreachable!("expected a checked comparison"),
        });
    }
    // SAFETY: the original operands and operation remain live for Python dispatch.
    unsafe { imre_dyn_op_at(ctx, opcode, left, right, NONE, std::ptr::null()) }
}

/// Apply checked remainder by a positive power of two to a Python operand.
///
/// # Safety
/// Generated code supplies its active context, a rooted dividend, and the
/// permanent integer handle of the proven literal divisor.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_masked_remainder(
    ctx: *mut Context,
    dividend: Id,
    divisor: Id,
) -> Id {
    // SAFETY: the generated frame owns the context and both operands.
    let context = unsafe { &*ctx };
    if context.heap.execution.pending.is_some() {
        return NONE;
    }
    if let Value::Int(integer) = context.heap.get(dividend)
        && let Some(value) = integer.as_small()
    {
        let zero = Heap::cached_integer(0).expect("zero has a permanent handle");
        let mask = divisor - zero - 1;
        return zero + ((value as Id) & mask);
    }
    // SAFETY: the original operands and checked frame are live on the fallback edge.
    unsafe {
        imre_dyn_op_at(
            ctx,
            c"%".as_ptr(),
            dividend,
            divisor,
            NONE,
            std::ptr::null(),
        )
    }
}

/// Read a subscription whose checked index produced an owned integer or a Python result.
///
/// # Safety
/// The caller transfers the nonzero integer word and roots the receiver and alternate
/// key. The position is null or points to the registered operation's static range.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_get_int_item(
    ctx: *mut Context,
    receiver: Id,
    word: usize,
    fallback: Id,
    span: *const [u32; 4],
) -> Id {
    // SAFETY: the checked frame owns the context and supplies its static range.
    let (ctx, span) = unsafe { (&mut *ctx, span.as_ref()) };
    let index = (word != 0).then(|| crate::Int(word));
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let key = if let Some(index) = index {
        if let Some(item) = ctx.exact_sequence_item(receiver, &index) {
            return finish_at(ctx, Ok(item), span);
        }
        let object = ctx.alloc(Value::Int(index));
        let object = finish(ctx, Ok(object));
        if ctx.heap.execution.pending.is_some() {
            return NONE;
        }
        object
    } else {
        fallback
    };
    let result = ctx.get_item(receiver, key);
    finish_at(ctx, result, span)
}

/// Move one owned tagged integer into the active Python heap.
///
/// # Safety
/// The caller transfers a live integer reference and owns the context for this call.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_take_int(ctx: *mut Context, word: usize) -> Id {
    // SAFETY: the caller owns the context and transfers the reference to the heap.
    let ctx = unsafe { &mut *ctx };
    let object = ctx.alloc(Value::Int(crate::Int(word)));
    finish(ctx, Ok(object))
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

/// Load a literal only after the preceding checked operation succeeded.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_boxed_guarded_lit(
    ctx: *mut Context,
    kind: i32,
    data: *const u8,
    len: usize,
) -> Id {
    // SAFETY: compiled code holds the context and supplies exactly len literal bytes.
    unsafe {
        if imre_dyn_failed(ctx) {
            0
        } else {
            imre_dyn_lit(ctx, kind, data, len)
        }
    }
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
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let mut result = std::str::from_utf8(name)
        .map_err(|error| Error::new("UnicodeError", error.to_string()))
        .and_then(|name| ctx.lookup(scope, name));
    finish_operand_position(ctx, &mut result, None);
    finish(ctx, result)
}

/// Read a checked target from its owning comprehension scope, including a captured cell.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_lookup_local(
    ctx: *mut Context,
    scope: Id,
    name: *const u8,
    len: usize,
) -> Id {
    // SAFETY: the emitter links a Rust str identifier of exactly len bytes.
    let (ctx, key) = unsafe {
        (
            &mut *ctx,
            std::str::from_utf8_unchecked(slice::from_raw_parts(name, len)),
        )
    };
    if ctx.heap.execution.pending.is_none()
        && ctx.scope_data(scope).namespace.is_none()
        && let Some(value) = ctx.scope_name(scope, key)
    {
        ctx.heap.take_call_position();
        return finish(ctx, Ok(value));
    }
    // SAFETY: the ordinary lookup accepts the same scope and identifier.
    unsafe { imre_dyn_lookup(ctx, scope, name, len) }
}

/// Store a linked identifier and report whether its previous owner needs releasing.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_assign_target(
    ctx: *mut Context,
    scope: Id,
    name: *const u8,
    len: usize,
    value: Id,
) -> Id {
    // SAFETY: the compiled frame supplies its active context and the linked identifier bytes.
    let (ctx, name) = unsafe { (&mut *ctx, slice::from_raw_parts(name, len)) };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = std::str::from_utf8(name)
        .map_err(|error| Error::new("UnicodeError", error.to_string()))
        .and_then(|name| ctx.store_source_name(scope, name, value))
        .map(flag);
    finish(ctx, result)
}

/// Execute an import statement through the importing frame's builtins.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_import_name(
    ctx: *mut Context,
    scope: Id,
    name: Id,
    fromlist: Id,
    level: u32,
) -> Id {
    // SAFETY: generated code passes its current interpreter and live Python handles.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = ctx.import_in_scope(ctx.heap.current_code(), scope, name, fromlist, level);
    finish(ctx, result)
}

/// Read a contiguous slice and attribute failures to the subscription expression.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_get_slice_at(
    ctx: *mut Context,
    receiver: Id,
    start: Id,
    stop: Id,
    span: *const [u32; 4],
) -> Id {
    // SAFETY: all original handles are forwarded without a linked constant marker.
    unsafe { imre_dyn_get_linked_slice_at(ctx, receiver, start, stop, std::ptr::null(), span) }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_get_linked_slice_at(
    ctx: *mut Context,
    receiver: Id,
    start: Id,
    stop: Id,
    marker: *const u8,
    span: *const [u32; 4],
) -> Id {
    // SAFETY: generated native code owns its context and supplies a static span or null.
    let (ctx, span) = unsafe { (&mut *ctx, span.as_ref()) };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = ctx.get_slice(
        receiver,
        start,
        stop,
        (!marker.is_null()).then_some(marker as usize),
    );
    finish_at(ctx, result, span)
}

/// Apply a boxed operation with a linked code name as its second operand.
#[unsafe(no_mangle)]
#[inline(never)]
pub(super) unsafe extern "C" fn imre_boxed_named_op(
    ctx: *mut Context,
    op: *const c_char,
    receiver: Id,
    name: *const u8,
    len: usize,
    value: Id,
) -> Id {
    // SAFETY: the emitter supplies a live context and a terminated opcode.
    if unsafe { !imre_dyn_failed(ctx) && libc::strcmp(op, c"setattr".as_ptr()) == 0 } {
        // The existing setter can consume this spelling without boxing the name.
        // SAFETY: the generated name has exactly len bytes for the lifetime of this call.
        let (ctx, name) = unsafe { (&mut *ctx, slice::from_raw_parts(name, len)) };
        let result = std::str::from_utf8(name)
            .map_err(|error| Error::new("UnicodeError", error.to_string()))
            .and_then(|name| ctx.set_attr(receiver, name, value));
        return finish(ctx, result);
    }
    // SAFETY: generated code owns the context, opcode and linked name for this call.
    unsafe {
        let key = imre_dyn_lit(ctx, 6, name, len);
        if imre_dyn_failed(ctx) {
            0
        } else {
            imre_dyn_op_at(ctx, op, receiver, key, value, std::ptr::null())
        }
    }
}

/// Preserve a pending exception before loading the operation's linked name.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_boxed_guarded_named_op(
    ctx: *mut Context,
    op: *const c_char,
    receiver: Id,
    name: *const u8,
    len: usize,
    value: Id,
) -> Id {
    // SAFETY: the guarded entry borrows the same inputs as imre_boxed_named_op.
    unsafe {
        if imre_dyn_failed(ctx) {
            0
        } else {
            imre_boxed_named_op(ctx, op, receiver, name, len, value)
        }
    }
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
    if ctx.heap.execution.pending.is_some() && op != "take_error" {
        return NONE;
    }
    let reraise = op == "reraise" && (a != NONE || ctx.heap.active_exception() != NONE);
    let mut result = ctx.op(op, a, b, c);
    let uniform_operand = matches!(
        op,
        "pos" | "neg" | "~" | "not" | "format" | "format_conversion"
    );
    if matches!(
        op,
        "attr"
            | "method"
            | "spread_arguments"
            | "extend"
            | "keywords"
            | "class_keywords"
            | "iter"
            | "await_iter"
            | "async_exit_context"
    ) || (b == TRUE && matches!(op, "lookup_enter" | "lookup_exit"))
        || uniform_operand
    {
        finish_operand_position(
            ctx,
            &mut result,
            uniform_operand.then_some(value::TracebackAnchor::Uniform),
        );
    }
    if reraise && let Err(error) = result {
        ctx.heap.execution.pending = Some(error);
        return NONE;
    }
    // SAFETY: the emitter supplies null or a native [4 x i32] module constant.
    let span = unsafe { span.as_ref() };
    if let Err(error) = &mut result
        && let Some(&[line, start, operator, end]) = span
        && let Some(width) = match op {
            "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "@" => Some(1),
            "//" | "**" | "<<" | ">>" => Some(2),
            _ => None,
        }
    {
        error.tracing().source_position = Some((
            line,
            value::TracebackSpan {
                start,
                end,
                end_line: line,
                anchor: value::TracebackAnchor::Operator(operator, width),
            },
        ));
        return finish(ctx, result);
    }
    if result.is_err() && span.is_none() && matches!(op, "//" | "%" | "<<" | ">>") {
        ctx.heap.use_expression_line();
    }
    finish_at(ctx, result, span)
}

/// Test the operand at its source position, preserving exceptions raised by Python truth hooks.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_truth_at(
    ctx: *mut Context,
    value: Id,
    span: *const [u32; 4],
) -> bool {
    // SAFETY: the caller owns its context and supplies null or a static module position.
    let (ctx, span) = unsafe { (&mut *ctx, span.as_ref()) };
    if ctx.heap.execution.pending.is_some() {
        return false;
    }
    let mut result = ctx.truth(value);
    finish_operand_position(ctx, &mut result, None);
    result.unwrap_or_else(|error| {
        finish_at(ctx, Err(error), span);
        false
    })
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
    unsafe {
        scoped_call(
            ctx,
            span.as_ref(),
            mark,
            callable,
            |ctx| {
                let arguments = ctx.items(args)?;
                if kwargs == NONE {
                    ctx.prepare_call_in_scope(scope, callable, arguments, Vec::new())
                } else {
                    ctx.prepare_call_with_keyword_dict(scope, callable, arguments, kwargs)
                }
            },
            |ctx, mark, result| {
                if !ctx.heap.discard_scalar_call(mark, result, args, callable) {
                    ctx.finish_call_operands(mark, result);
                }
            },
        )
    }
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
    method_target(ctx, bound_receiver, |ctx| {
        ctx.string_method_target(receiver, name)
    })
}

/// Prepare an ordinary positional method call before evaluating its arguments.
///
/// # Safety
/// The caller supplies the active context, a NUL-terminated UTF-8 name and a writable receiver slot.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_builtin_method_target(
    ctx: *mut Context,
    receiver: Id,
    name: *const c_char,
    bound_receiver: *mut Id,
) -> Id {
    // SAFETY: generated code retains the context and passes static method names.
    let (ctx, name, bound_receiver) = unsafe { (&mut *ctx, cstr(name), &mut *bound_receiver) };
    method_target(ctx, bound_receiver, |ctx| {
        ctx.builtin_method_target(receiver, name)
    })
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
    method_target(ctx, bound_receiver, |ctx| ctx.dict_get_target(receiver))
}

/// Propagate a failing receiver expression before looking up its method.
fn method_target(
    ctx: &mut Context,
    bound_receiver: &mut Id,
    lookup: impl FnOnce(&mut Context) -> Result<(Id, Id)>,
) -> Id {
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let mut result = lookup(ctx).map(|(callable, receiver)| {
        *bound_receiver = receiver;
        callable
    });
    finish_operand_position(ctx, &mut result, None);
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
    if ctx.heap.execution.pending.is_some() {
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
        return unsafe {
            scoped_call(
                context,
                span.as_ref(),
                mark,
                callable,
                |ctx| {
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
                    Ok(calls::RegisteredCall::Returned(selected))
                },
                Context::finish_call_operands,
            )
        };
    }
    // SAFETY: the receiver slot, argument and call position follow the prepared method ABI.
    let result = unsafe {
        imre_dyn_positional_method_at(context, callable, operands.cast(), 2, mark, span, scope)
    };
    // SAFETY: the call has returned, so its borrow of this context has ended.
    let ctx = unsafe { &mut *context };
    if ctx.heap.execution.pending.is_some() {
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
    unsafe {
        scoped_call(
            ctx,
            span.as_ref(),
            mark,
            callable,
            |ctx| {
                let args = copy_ids(live_handles(values, count))?;
                ctx.prepare_call_in_scope(scope, callable, args, Vec::new())
            },
            Context::finish_call_operands,
        )
    }
}

/// Call with positional values followed by values for statically named keywords.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_keyword_call_at(
    ctx: *mut Context,
    callable: Id,
    values: *const Id,
    positional: usize,
    mut names: *const c_char,
    keywords: usize,
    mark: usize,
    span: *const [u32; 4],
    scope: Id,
) -> Id {
    // SAFETY: the compiler supplies its context, optional static position and initialized handles.
    unsafe {
        scoped_call(
            ctx,
            span.as_ref(),
            mark,
            callable,
            |ctx| {
                let values = live_handles(values, positional + keywords);
                let (args, named) = values.split_at(positional);
                let args = copy_ids(args)?;
                let mut kwargs = Vec::new();
                kwargs
                    .try_reserve_exact(keywords)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                for &value in named {
                    // SAFETY: the emitter stores one NUL-terminated UTF-8 identifier for each value.
                    let name = cstr(names);
                    kwargs.push((copy_label(name)?, value));
                    names = names.add(name.len() + 1);
                }
                ctx.prepare_call_in_scope(scope, callable, args, kwargs)
            },
            Context::finish_call_operands,
        )
    }
}

/// Preserve the operation range when transferring an exception back to the caller.
fn finish_operand_position<T>(
    ctx: &mut Context,
    result: &mut Result<T>,
    ordinary_anchor: Option<value::TracebackAnchor>,
) {
    if let Some([line, end_line, start, end]) = ctx.heap.take_call_position()
        && let Err(error) = result
    {
        let anchor = if matches!(
            ctx.code_object(ctx.heap.current_code()).body,
            value::Body::Suspended(_)
        ) {
            value::TracebackAnchor::Uniform
        } else if let Some(anchor) = ordinary_anchor {
            anchor
        } else {
            return;
        };
        error.tracing().source_position = Some((
            line,
            value::TracebackSpan {
                end_line,
                start,
                end,
                anchor,
            },
        ));
    }
}

/// # Safety
/// The generated caller owns execution of this context. Its operand mark and position
/// belong to the caller's frame and stay live while the registered frame runs.
unsafe fn scoped_call(
    context: *mut Context,
    span: Option<&[u32; 4]>,
    mark: usize,
    callable: Id,
    prepare: impl FnOnce(&mut Context) -> Result<calls::RegisteredCall>,
    release: impl FnOnce(&mut Context, usize, Id),
) -> Id {
    // SAFETY: restrict preparation's borrow to its own phase of the call.
    let ctx = unsafe { &mut *context };
    if ctx.heap.execution.pending.is_some() {
        ctx.heap.set_call_position(None);
        return NONE;
    }
    // SAFETY: the generated caller owns the marked operands throughout the call.
    let mut result = unsafe { run_with_operands(context, mark, callable, prepare, release) };
    // SAFETY: execution has returned to the generated caller.
    let ctx = unsafe { &mut *context };
    // Ordinary traceback nodes also own the saved call range, including when the
    // emitter supplied no highlight for the traceback printer.
    finish_operand_position(ctx, &mut result, Some(value::TracebackAnchor::Hidden));
    finish_at(ctx, result, span)
}

/// Run a prepared call with the normal caller-to-frame operand transfer.
///
/// # Safety
/// The current thread owns this context and its marked caller frame through return.
#[inline]
pub(super) unsafe fn run_with_operands(
    context: *mut Context,
    mark: usize,
    callable: Id,
    prepare: impl FnOnce(&mut Context) -> Result<calls::RegisteredCall>,
    release: impl FnOnce(&mut Context, usize, Id),
) -> Result<Id> {
    // SAFETY: borrow the caller only for preparation, before invoking native code.
    let ctx = unsafe { &mut *context };
    let call = prepare(ctx);
    let transferred = matches!(call, Ok(calls::RegisteredCall::Native(_)))
        && match *ctx.heap.get(callable) {
            Value::Function { .. } => true,
            Value::Method { callable, .. } => {
                matches!(ctx.heap.get(callable), Value::Function { .. })
            }
            _ => false,
        };
    if transferred {
        // Binding succeeded. The native frame now owns the receiver and argument values.
        ctx.heap.release_bound_call_operands(mark);
    }
    // SAFETY: preparation returned any native activation with its frame already rooted.
    let result = call.and_then(|call| unsafe { call.run(context) });
    // SAFETY: the native frame has returned and the caller is active again.
    let ctx = unsafe { &mut *context };
    result.and_then(|value| {
        if transferred {
            // Caller-side packed handles may already have been collected.
            ctx.finish_call_operands(mark, value);
        } else {
            release(ctx, mark, value);
        }
        ctx.check_call_return(value)
    })
}

/// Definition sites carry either an ordinary entry or a coroutine resume entry.
#[repr(C)]
union PrelinkedEntry {
    native: Code,
    suspended: value::ResumeCode,
}

#[repr(C)]
pub(super) struct PrelinkedSite {
    entry: PrelinkedEntry,
    // Kept in the linked site ABI. The active parent Code owns the filename.
    _filename: *const c_char,
    first_line: u32,
    tag: u32,
}

/// Definition-time scope and flag fields from a compiled function's parameter metadata.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct DefinitionMetadata {
    generator_lambda: bool,
    annotation_class: serde_json::Value,
    scope_reads: serde_json::Value,
    scope_complete: serde_json::Value,
    method_class_cell: serde_json::Value,
    class_cell: serde_json::Value,
    code_flags: serde_json::Value,
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
    let owner = ctx.heap.current_code();
    let code = if let Some(&(_, code)) = ctx
        .code_object(owner)
        .nested
        .iter()
        .find(|(site, _)| *site == key)
    {
        code
    } else {
        let filename = ctx.code_object(owner).filename;
        // SAFETY: the emitter initializes the union with the function ABI selected by the tag.
        let body = unsafe {
            match definition.tag {
                0 => value::Body::Native(definition.entry.native, NONE),
                value::NATIVE_SUSPENDED_BODY => value::Body::Suspended(definition.entry.suspended),
                _ => {
                    return finish(
                        ctx,
                        Err(Error::new("SystemError", "invalid compiled definition tag")),
                    );
                }
            }
        };
        let code = ctx.prelinked_code(
            body,
            scope,
            name,
            parameters,
            (qualname, doc),
            (filename, definition.first_line),
        );
        let constants = match ctx.heap.compilation_constants(owner) {
            Ok(constants) => constants,
            Err(error) => return finish(ctx, Err(error)),
        };
        let Value::Code(parent) = ctx.heap.get_mut(owner) else {
            unreachable!()
        };
        parent.nested.push((key, code));
        let Value::Code(child) = ctx.heap.get_mut(code) else {
            unreachable!()
        };
        child.compilation_constants = constants;
        code
    };
    let result = ctx.define_function(
        code,
        scope,
        name,
        defaults,
        qualname,
        doc,
        annotate,
        annotation_scope,
    );
    finish(ctx, result)
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

    /// Include the explicit or implicit __class__ read once in the sorted closure layout.
    fn freevars_with_class_cell<'a>(
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
            // Class and mapping locals serve direct reads. A nested function closes over their parent.
            if !frame.in_class_body && frame.exec_locals.is_none() {
                if frame.globals.contains(name) {
                    return Some(false);
                }
                if frame.closure.declared.contains(name) || frame.closure.cells.contains_key(name) {
                    return Some(true);
                }
                if !frame.closure.declared_complete {
                    return None;
                }
                if name == "__class__" && frame.class_cell.is_some() {
                    return Some(true);
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
            if !frame.in_class_body && frame.closure.declared.contains(name) {
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
            if !frame.in_class_body
                && name == "__class__"
                && let Some(cell) = frame.class_cell
            {
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
        (filename, first_line): (Id, u32),
    ) -> Id {
        let (qualname, doc) = metadata_pair;
        let module_definition = self.scope_data(scope).parent.is_none();
        let metadata: DefinitionMetadata = self
            .str_value(parameters)
            .ok()
            .filter(|v| v.trim_start().starts_with('{'))
            .and_then(|v| serde_json::from_str(v).ok())
            .unwrap_or_default();
        let annotation_class = metadata.annotation_class == true;
        let class = self.scope_data(scope);
        let class_reads =
            class.in_class_body && (annotation_class || metadata.method_class_cell.is_boolean());
        let mut implicit = Vec::new();
        if annotation_class {
            implicit.push("__classdict__".to_owned());
        }
        let reads = metadata
            .scope_reads
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
            .filter(|&name| {
                if annotation_class && name == "__classdict__" {
                    return false;
                }
                let captured = match name {
                    "__class__" => annotation_class && class.in_class_body,
                    "__classdict__" => class_reads,
                    CONDITIONAL => class_reads && class.conditional_annotation_cell().is_some(),
                    _ => false,
                };
                if captured {
                    implicit.push(name.to_owned());
                }
                !captured
            });
        let lexical = if annotation_class
            && !self
                .scope_data(scope)
                .closure
                .cells
                .contains_key("__classdict__")
        {
            self.scope_data(scope).parent.unwrap_or(scope)
        } else {
            scope
        };
        let freevars = if metadata.scope_complete != true {
            None
        } else if let Some(captures_class) = metadata.method_class_cell.as_bool() {
            self.freevars_with_class_cell(self.scope_data(scope).parent, reads, captures_class)
        } else if metadata.class_cell == true {
            // A nested function can read __class__ through super(), or resolve it as a global.
            self.is_enclosing_freevar(lexical, "__class__")
                .and_then(|captures| self.freevars_with_class_cell(Some(lexical), reads, captures))
        } else {
            self.enclosing_freevars(lexical, reads)
        }
        .map(|mut freevars| {
            if !implicit.is_empty() {
                freevars.append(&mut implicit);
                freevars.sort();
            }
            freevars
        });
        let closure_supported = freevars.is_some();
        let freevars = freevars.unwrap_or_default();
        let flags = metadata
            .code_flags
            .as_u64()
            .and_then(|v| u32::try_from(v).ok());
        let definition = value::CodeDefinition {
            module_definition,
            flags: flags.unwrap_or_default(),
            flags_complete: flags.is_some(),
            generator_lambda: metadata.generator_lambda,
            freevars: (!freevars.is_empty()).then(|| freevars.into()),
            closure_supported,
        };
        self.realize_code(
            body,
            definition,
            name,
            parameters,
            (qualname, doc),
            (filename, first_line),
        )
    }

    pub(super) fn realize_code(
        &mut self,
        body: value::Body,
        definition: value::CodeDefinition,
        name: Id,
        parameters: Id,
        (qualname, doc): (Id, Id),
        (filename, first_line): (Id, u32),
    ) -> Id {
        self.alloc(Value::Code(value::CodeObject {
            compilation_constants: NONE,
            parsed_parameters: Default::default(),
            varnames: NONE,
            cellvars: NONE,
            freevar_names: NONE,
            body,
            name,
            qualname,
            doc,
            parameters,
            filename,
            first_line,
            definition,
            nested: Vec::new(),
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
    ) -> Result<Id> {
        let defining_scope = scope;
        let owner = self.scope_data(scope);
        let in_class = owner.in_class_body;
        let conditional_cell = owner.conditional_annotation_cell();
        // A borrowed method only owns its defining class if its code captures __class__.
        let code_data = self.code_object(code);
        let mut class_cell = owner.class_cell.filter(|_| {
            !code_data.definition.closure_supported
                || code_data
                    .definition
                    .freevars()
                    .iter()
                    .any(|name| name == "__class__")
        });
        let scope = if owner.exec_locals.is_some()
            || ((owner.in_class_body || owner.class_namespace.is_some())
                && !annotation_scope)
        {
            owner.parent.unwrap_or(scope)
        } else {
            scope
        };
        let root = self.root_scope(scope);
        let fields = super::function_fields::FunctionFields::new(
            self, scope, root, name, qualname, doc, annotate,
        );
        let names = self.code_object(code).definition.freevars.clone();
        let names = names.as_deref().unwrap_or_default();
        let entries = if defaults == NONE {
            Vec::new()
        } else {
            self.dict_entries(defaults)
                .expect("function defaults are a dict")
                .to_vec()
        };
        // Only default values and implicit class cells need the parameter metadata here.
        let signature = self.code_object(code).parameters;
        let signature: serde_json::Value = if signature == NONE
            || (entries.is_empty()
                && !names.iter().any(|name| {
                    matches!(name.as_str(), "__class__" | "__classdict__" | CONDITIONAL)
                })) {
            serde_json::Value::Null
        } else {
            serde_json::from_str(
                self.str_value(signature)
                    .expect("function parameters are text"),
            )
            .expect("function parameters are valid JSON")
        };
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
        let mut cells = indexmap::IndexMap::with_capacity(names.len());
        let class_reads = in_class
            && (signature["annotation_class"] == true
                || signature["method_class_cell"].is_boolean());
        for name in names {
            let cell = if name == "__classdict__"
                && (class_reads || signature["annotation_class"] == true)
            {
                let owner = if class_reads { defining_scope } else { scope };
                self.class_annotation_dict_cell(owner)
            } else if name == CONDITIONAL
                && class_reads
                && let Some(cell) = conditional_cell
            {
                cell
            } else if name == "__class__"
                && in_class
                && (signature["method_class_cell"] == true || signature["annotation_class"] == true)
            {
                class_cell.expect("class scope")
            } else {
                let cell = self.capture_free_cell(scope, name);
                if name == "__class__" {
                    class_cell = Some(cell);
                }
                cell
            };
            cells.insert(name.clone(), cell);
        }
        let scope = if self.code_object(code).definition.closure_supported {
            let globals = self.new_function_globals(root)?;
            self.environment_with_cells(globals, cells)
        } else if self.scope_data(scope).parent.is_none() {
            self.new_function_globals(scope)?
        } else {
            scope
        };
        Ok(self.alloc(Value::Function {
            fields,
            closure: NONE,
            code,
            scope,
            class_cell,
            defaults,
            kwdefaults,
        }))
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_failed(ctx: *const Context) -> bool {
    // SAFETY: the generated caller holds the context throughout this query.
    unsafe { (*ctx).heap.execution.pending.is_some() }
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

/// Check the result discarded by a compiled expression statement after evaluating it once.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_discard_expression(ctx: *mut Context, value: Id) {
    // SAFETY: the caller owns the context and the result is live in its current frame.
    unsafe { (&mut *ctx).heap.note_discarded_coroutine(value) }
}

/// Release a consumed truth operand on the branch that no longer needs it.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_discard_truth(ctx: *mut Context, mark: usize) {
    // SAFETY: the mark comes from the same active Python frame.
    unsafe { (&mut *ctx).discard_truth(mark) }
}

/// Unwind a partially evaluated operand stack from the top, keeping the exception live.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_unwind_operands(ctx: *mut Context, mark: usize) {
    // SAFETY: the generated caller owns this context and marked the same activation.
    unsafe { (&mut *ctx).unwind_operands(mark) }
}

/// Construct a template interpolation from its four evaluated Python objects.
///
/// # Safety
/// The generated frame owns the context and roots all four handles.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_interpolation(
    ctx: *mut Context,
    value: Id,
    expression: Id,
    conversion: Id,
    format_spec: Id,
) -> Id {
    // SAFETY: the generated caller owns the active context.
    let ctx = unsafe { &mut *ctx };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = ctx.template_interpolation(value, expression, conversion, format_spec);
    finish(ctx, result)
}

/// Join the checked f-string parts and pop them in Python evaluation-stack order.
///
/// # Safety
/// The generated caller owns the context, retains the readable array of `count`
/// handles, and took `mark` before evaluating this expression.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_format_parts(
    ctx: *mut Context,
    mark: usize,
    values: *const Id,
    count: usize,
) -> Id {
    // SAFETY: the caller owns the context and roots the parts through the join.
    let (ctx, values) = unsafe { (&mut *ctx, live_handles(values, count)) };
    // Successful joins add only exact text roots. Release those with the last source
    // operand, keeping the joined result live before a source destructor can run.
    let parts_end = ctx.heap.temporary_mark();
    let joined = ctx.join_formatted(values);
    let result = finish(ctx, joined);
    if ctx.heap.execution.pending.is_none() {
        if ctx.heap.observes_released_roots() {
            for slot in (mark + 1..parts_end).rev() {
                ctx.discard_temporaries(slot, [result]);
            }
        }
        ctx.discard_temporaries(mark, [result]);
    }
    result
}

/// Publish surviving operands before collecting, returning the boundary above the first one.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_keep_operands(
    ctx: *mut Context,
    mark: usize,
    first: Id,
    second: Id,
) -> usize {
    // SAFETY: these live handles and the mark belong to the current generated frame.
    let ctx = unsafe { &mut *ctx };
    ctx.discard_temporaries(
        mark,
        std::iter::once(first).chain((second != NONE).then_some(second)),
    );
    mark + 1
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

/// Unwind a failed statement after its exception has been taken for a context exit.
///
/// # Safety
/// Generated code supplies the active frame's surviving roots, including the
/// exception and exit callable, then its abandoned operands from bottom to top.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_unwind_frame(
    ctx: *mut Context,
    live: *const Id,
    len: usize,
    retained: usize,
) {
    // SAFETY: the caller owns the context and provides len initialized handles.
    let (ctx, live) = unsafe { (&mut *ctx, live_handles(live, len)) };
    debug_assert!(retained <= len && ctx.heap.execution.pending.is_none());
    ctx.heap.insert_temporaries(0, live);
    ctx.unwind_operands(retained);
}

/// Unwind a failed expression's operands before its pending lexical targets.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_unwind_slots(
    ctx: *mut Context,
    mark: usize,
    retiring: *const Id,
    len: usize,
) {
    // SAFETY: generated code passes initialized root words and its own operand mark.
    let (ctx, retiring) = unsafe { (&mut *ctx, live_handles(retiring, len)) };
    if ctx.heap.observes_released_roots() {
        ctx.heap.insert_temporaries(mark, retiring);
    }
    ctx.unwind_operands(mark);
}

/// Finish a store while retaining the operands below its mark.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_assignment_target_at(
    ctx: *mut Context,
    mark: usize,
    live: *const Id,
    len: usize,
    replaced: Id,
) {
    // SAFETY: the caller owns the context and its live handle array.
    let (ctx, live) = unsafe { (&mut *ctx, live_handles(live, len)) };
    if ctx.heap.assignment_target(mark, live, replaced == TRUE)
        && !ctx.can_skip_finalizer_sweep(ctx.heap.temporary_mark())
    {
        ctx.collect_after_release();
    }
}

/// Expose a compound statement's full instruction position during its implicit calls.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_definition_position(
    ctx: *mut Context,
    position: *const [u32; 4],
) {
    // SAFETY: linked code supplies its active context and an optional static range.
    let (ctx, position) = unsafe { (&mut *ctx, position.as_ref()) };
    ctx.heap.set_definition_position(position.copied());
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

/// Native code entry for the namedtuple constructor, after ordinary Python argument binding.
pub(super) unsafe extern "C" fn namedtuple_constructor(raw: *mut Context, scope: Id) -> Id {
    // SAFETY: execute entered the function frame; all arguments are rooted by its scope and
    // the static filename lives for the whole process. There are no operand temporaries yet.
    unsafe {
        imre_dyn_statement_with_exception(raw, std::ptr::null(), 0, c"<string>".as_ptr(), 1, NONE)
    };
    // SAFETY: the caller retains exclusive ownership until this code entry returns.
    let ctx = unsafe { &mut *raw };
    if ctx.heap.execution.pending.is_some() {
        return NONE;
    }
    let result = super::namedtuple::construct(ctx, scope);
    finish(ctx, result)
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
    // SAFETY: emitted filenames are static native strings for the life of the executable.
    if let Err(error) = unsafe { ctx.native_statement(live, file, line, caught) } {
        finish(ctx, Err(error));
    }
}

impl Context {
    /// Publish a native statement. Its caller records a failure once at the frame boundary.
    pub(super) unsafe fn native_statement(
        &mut self,
        live: &[Id],
        file: *const c_char,
        line: u32,
        caught: Id,
    ) -> Result<()> {
        // SAFETY: both the emitted ABI and native providers pass a static native filename.
        let file = unsafe { self.native_filename_label(file) }?;
        self.checkpoint(
            live,
            value::Location {
                file,
                line,
                span: None,
            },
            caught,
        );
        #[cfg(unix)]
        // SAFETY: the statement has published this frame and its live operands.
        unsafe { Self::dispatch_pending_calls(self)? };
        Ok(())
    }

    /// Decode a static, NUL-terminated native filename once for statement diagnostics.
    unsafe fn native_filename_label(&mut self, file: *const c_char) -> Result<Arc<str>> {
        if let Some(label) = self.native_filename_labels.get(&file.addr()) {
            return Ok(label.clone());
        }
        // SAFETY: the emitted filename is a native path constant for the lifetime of the program.
        let path = unsafe { native_path(file) };
        let label: Arc<str> = match path.to_str() {
            Some(text) if text.is_ascii() => text.into(),
            _ => {
                let filename = self.import_path_text(path)?;
                self.filename_display(filename)?.as_ref().into()
            }
        };
        self.native_filename_labels
            .insert(file.addr(), label.clone());
        Ok(label)
    }

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
        self.workers.reap_finished(&mut self.heap);
        (
            &mut self.heap,
            self.classes
                .values()
                .chain(self.enum_scopes.values())
                .chain(self.signal_handlers.iter())
                .chain(self.displaced_sni_callbacks.iter())
                .chain(self.typing.roots())
                .chain(self.builtins.values())
                .chain(self.native_type_dicts.values())
                .chain(self.warning_state.iter().flat_map(|state| state.roots()))
                .chain(self.codec_entries.roots())
                .chain(&self.audit_hooks)
                .chain(self.fork_hooks.iter().flatten())
                .chain(&self.exit_callbacks)
                .copied()
                .chain(self.workers.roots().copied())
                .chain(self.threads.roots())
                .chain(self.fault_file)
                .chain(self.fault_user_files.values().copied())
                .chain(self.contexts.iter().copied())
                .chain(self.buffer_exports.iter().copied())
                .chain(self.original_builtins)
                .chain(self.source_builtins)
                .chain([
                    self.argv,
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
            || (finalizers
                && !self.can_skip_finalizer_sweep(self.heap.temporary_mark())
                && !self.surviving_release_roots())
        {
            let (heap, roots) = self.heap_and_roots();
            if let Some(callbacks) = heap.collect(roots) {
                self.run_weak_callbacks(callbacks);
            }
            let _ = self.drain_scheduled_coroutines();
        }
    }

    /// Flush unreachable file owners before the caller evaluates its next operand.
    pub(super) fn leave_frame(&mut self, result: Id, returning: bool) {
        if self.heap.leave(result, returning)
            && !self.can_skip_finalizer_sweep(self.heap.temporary_mark())
        {
            if self.finalize_unowned_generator_loop()
                && self.can_skip_finalizer_sweep(self.heap.temporary_mark())
            {
                return;
            }
            if self.heap.retire_closed_descriptors()
                && self.can_skip_finalizer_sweep(self.heap.temporary_mark())
            {
                return;
            }
            self.collect_after_release();
        }
    }

    /// Close an isolated generator before paying for a complete collection at frame exit.
    fn finalize_unowned_generator_loop(&mut self) -> bool {
        let Some((generator, iterator)) = self.heap.pending_generator_loop() else {
            return false;
        };
        let globals = self.finalizer_path_roots();
        let streams = super::stdlib::io::rooted_standard_streams(self);
        if !self.heap.cached_finalizers_kept_except(
            globals,
            streams.as_ref(),
            Some(self.heap.temporary_mark()),
            &[generator],
        ) {
            return false;
        }
        let (heap, roots) = self.heap_and_roots();
        if !heap.generator_loop_is_unowned(generator, iterator, roots) {
            return false;
        }
        self.dispatch_finalizer(generator);
        true
    }

    /// Release a removed list binding while its remaining items still own their references.
    pub(super) fn release_list_owner(&mut self, owner: Id) {
        if !self.heap.observes_released_roots()
            || !matches!(self.heap.get(owner), Value::List(items) if !items.is_empty())
            || (!self.heap.has_weak_values && !self.heap.could_own_finalizer(owner, false))
        {
            return;
        }
        let mark = self.heap.temporary_mark();
        if self.can_skip_finalizer_sweep(mark) {
            return;
        }
        self.heap.keep(owner);
        let (heap, roots) = self.heap_and_roots();
        if heap.expand_released_sequence(mark, roots) {
            self.unwind_operands(mark);
        } else {
            self.heap.release_temporaries_since(mark);
        }
    }

    /// Release expression operands from the stack top. A dead sequence transfers its
    /// elements to the stack, keeping earlier siblings alive across Python callbacks.
    pub(super) fn unwind_operands(&mut self, mark: usize) {
        if !self.heap.observes_released_roots() || self.can_skip_finalizer_sweep(mark) {
            self.discard_truth(mark);
            return;
        }
        while self.heap.temporary_mark() > mark {
            let index = self.heap.temporary_mark() - 1;
            let (heap, roots) = self.heap_and_roots();
            if !heap.expand_released_sequence(index, roots) {
                self.discard_truth(index);
            }
        }
    }

    /// Publish the surviving roots before the next operand can observe a discarded writer.
    pub(super) fn discard_truth(&mut self, mark: usize) {
        if let Some(error) = self.heap.execution.pending.take() {
            // A weak callback may reenter Python during unwind. Keep the original
            // exception alive without exposing it as a failure of the callback.
            self.discard_temporaries(mark, error.roots());
            self.heap.execution.pending = Some(error);
            self.heap.release_temporaries_since(mark);
        } else {
            self.discard_temporaries(mark, None);
        }
    }

    /// Release operand roots and report whether a weak callback or finalizer may become runnable.
    fn release_operand_roots(&mut self, mark: usize) -> bool {
        if (!self.heap.tracks_finalizers() && !self.heap.has_weak_values)
            || self.can_skip_finalizer_sweep(mark)
        {
            self.heap.release_temporaries_since(mark);
            false
        } else {
            self.heap.discard_truth_operands(mark, self.modules)
        }
    }

    /// Root the surviving results before an operand release runs weak callbacks or finalizers.
    pub(super) fn discard_temporaries(
        &mut self,
        mark: usize,
        results: impl IntoIterator<Item = Id>,
    ) {
        let collect = self.release_operand_roots(mark);
        for result in results {
            self.heap.keep(result);
        }
        if collect {
            self.collect_after_release();
        }
    }

    /// Release temporary operands while their return value or raised exception stays live.
    pub(super) fn release_with_result(&mut self, mark: usize, result: Result<Id>) -> Result<Id> {
        let collect = self.release_operand_roots(mark);
        match &result {
            Ok(value) => self.heap.keep(*value),
            Err(error) => {
                for root in error.roots() {
                    self.heap.keep(root);
                }
            }
        }
        if collect {
            self.collect_after_release();
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

    /// The caller already discarded its dead operands and found a possible observable release.
    fn surviving_release_roots(&mut self) -> bool {
        if !self.heap.has_finalizer_cohort() {
            return false;
        }
        let (heap, roots) = self.heap_and_roots();
        heap.retained_paths_keep_owners(roots, true)
    }

    fn collect_after_release(&mut self) {
        if self.surviving_release_roots() {
            self.learn_release_paths();
        } else {
            self.collect_unreachable();
        }
    }

    /// Skip the sweep only while surviving roots keep every finalizer and weak edge alive.
    pub(super) fn can_skip_finalizer_sweep(&self, retained_current: usize) -> bool {
        if self.heap.full_collection_active() {
            return false;
        }
        let streams = super::stdlib::io::rooted_standard_streams(self);
        if streams.as_ref().is_some_and(|roots| {
            self.heap.tracked_finalizer_count() <= super::value::MAX_FRAME_FINALIZERS + roots.len()
                && self.heap.finalizers_kept_by(roots)
        }) {
            return !self.heap.has_weak_values || {
                let [modules, reference] = self.finalizer_path_roots();
                self.heap.weak_values_kept_by(modules, reference)
            };
        }
        self.heap.cached_finalizers_kept(
            self.finalizer_path_roots(),
            streams.as_ref(),
            Some(retained_current),
        )
    }

    /// Keep the returned value and earlier operands while dropping the completed call's temporaries.
    pub(super) fn finish_call_operands(&mut self, mark: usize, result: Id) {
        if !self.heap.tracks_finalizers() {
            return;
        }
        if self.can_skip_finalizer_sweep(mark) {
            self.heap.keep_call_result(mark, result);
        } else if self.heap.discard_operands(mark, result, self.modules) {
            // The return value is now rooted in the caller, including its live stream layers.
            let stream = super::stdlib::io::text_descriptor_chain(&self.heap, result);
            if !self.heap.full_collection_active()
                && (stream.is_some()
                    || self.heap.is_pending_finalizer(result)
                    || matches!(self.heap.get(result), Value::File(_)))
                && self.heap.cached_finalizers_kept_except(
                    self.finalizer_path_roots(),
                    super::stdlib::io::rooted_standard_streams(self).as_ref(),
                    Some(self.heap.temporary_mark()),
                    stream
                        .as_ref()
                        .map_or(std::slice::from_ref(&result), |chain| chain.as_slice()),
                )
            {
                return;
            }
            self.collect_after_release();
        }
    }

    /// Finish an assignment that dropped a possible finalizer owner.
    pub(super) fn finish_binding_release(&mut self) {
        if !self.can_skip_finalizer_sweep(self.heap.temporary_mark()) {
            self.collect_after_release();
        }
        self.heap.release_unbound_finalizers();
    }

    /// Deliver warnings discovered by this sweep before the next Python operation changes filters.
    fn drain_scheduled_coroutines(&mut self) -> Result<()> {
        if !self.heap.full_collection_active() && self.heap.take_unawaited_check() {
            self.drain_acyclic_finalizers()?;
        }
        Ok(())
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
        let _ = self.drain_scheduled_coroutines();
        self.learn_release_paths();
    }

    fn learn_release_paths(&mut self) {
        let [modules, reference] = self.finalizer_path_roots();
        self.heap.learn_weak_roots(modules, reference);
        let streams = super::stdlib::io::rooted_standard_streams(self);
        self.heap
            .learn_descriptor_paths([modules, reference], streams.as_ref());
    }

    /// Resolve weakrefs before callbacks, then recheck the original cyclic isolate.
    pub(super) fn collect_generation(&mut self, generation: u8) -> Result<usize> {
        if self.heap.full_collection_active() {
            return Ok(0);
        }
        // A warning hook can resurrect a coroutine whose arguments include a separate cycle.
        // Resolve unreachable acyclic owners before clearing the cyclic isolate's weakrefs.
        if self.heap.has_unstarted_coroutines() {
            self.collect_unreachable();
        }
        self.drain_acyclic_finalizers()?;
        let mark = self.heap.temporary_mark();
        let (heap, roots) = self.heap_and_roots();
        let callbacks = heap.begin_collection(roots, generation)?;
        if callbacks.is_empty() && self.heap.finish_empty_collection() {
            self.drain_scheduled_coroutines()?;
            return Ok(0);
        }
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
        if generation < 2 {
            // Even a resurrected cycle can release its last edge to an older acyclic value.
            self.collect_unreachable();
        }
        self.heap.end_full_collection();
        self.drain_scheduled_coroutines()?;
        Ok(count + released)
    }

    /// Invoke pending callbacks after the heap has cleared every dead reference.
    pub(super) fn run_weak_callbacks(&mut self, callbacks: impl IntoIterator<Item = (Id, Id)>) {
        for (callback, reference) in callbacks {
            let mark = self.heap.temporary_mark();
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
            self.discard_truth(mark);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::{Rc, Weak};

    #[test]
    fn compiled_pull_uses_the_current_range_iterator() {
        use crate::Int;

        let mut ctx = new_context(serde_json::json!({"paths": []}), ["imre"]);
        for (start, reverse) in [(20_i128, false), (i128::from(i64::MAX) + 1, true)] {
            let range = range::Range::new(
                Int::new(start.into()),
                Int::new((start + 3).into()),
                Int::from(1),
            )
            .unwrap();
            let iter = ctx.alloc(Value::RangeIterator(range.iter(reverse)));
            // Advance the same iterator through a different reference before the loop.
            ctx.next_item(iter).unwrap().unwrap();
            let mut item = NONE;
            for expected in [start + 1, if reverse { start } else { start + 2 }] {
                // SAFETY: the iterator is live and the item slot is writable.
                let status = unsafe { imre_dyn_list_cursor_pull(&mut ctx, iter, &mut item, false) };
                assert_eq!(status, 1);
                assert_eq!(ctx.repr(item), expected.to_string());
            }
            // SAFETY: exhaustion uses the same live context and slot.
            let status = unsafe { imre_dyn_list_cursor_pull(&mut ctx, iter, &mut item, true) };
            assert_eq!(status, 0);
            assert!(ctx.heap.execution.pending.is_none());
        }
    }

    #[test]
    fn native_function_construction_uses_interpreter_builtins_without_mutating_globals() {
        unsafe extern "C" fn size(raw: *mut Context, scope: Id) -> Id {
            // SAFETY: the native function ABI supplies the active, exclusively borrowed Context.
            let ctx = unsafe { &mut *raw };
            let result = ctx.lookup(scope, "len").and_then(|callee| {
                let text = ctx.text("abc");
                ctx.call(callee, vec![text], Vec::new())
            });
            finish(ctx, result)
        }

        let mut ctx = new_context(serde_json::json!({"paths": []}), ["imre"]);
        let builtins = ctx.default_source_builtins().unwrap();
        let scope = ctx.scope("example", None);
        let name = ctx.text("size");
        let parameters = ctx.text(r#"{"scope_complete":true,"scope_reads":["len"]}"#);
        let filename = ctx.text("example.py");
        let code = ctx.prelinked_code(
            value::Body::Native(size, NONE),
            scope,
            name,
            parameters,
            (name, NONE),
            (filename, 1),
        );
        let globals = ctx.dict(Vec::new());
        let function = ctx.construct_function(&[code, globals], &[]).unwrap();
        assert_eq!(ctx.lookup_attr(function, "__builtins__").unwrap(), builtins);
        assert!(ctx.dict_named_item(globals, "__builtins__").is_none());
        let size = ctx.call(function, vec![], vec![]).unwrap();
        assert_eq!(ctx.as_int(size).unwrap().to_i64(), Some(3));
    }

    #[test]
    fn newly_created_functions_capture_rebound_builtins_in_the_same_globals() {
        let mut ctx = new_context(
            serde_json::json!({"paths": [], "python_version": "3.14"}),
            ["imre"],
        );
        let old = ctx.dict(Vec::new());
        let updated = ctx.dict(Vec::new());
        let key = ctx.text("__builtins__");
        let globals = ctx.dict(vec![(key, old)]);
        let first = ctx.function_environment("invoice", globals, old);
        assert_eq!(ctx.new_function_globals(first).unwrap(), first);
        ctx.set_dictionary_key(globals, key, updated, None, true)
            .unwrap();
        let second = ctx.new_function_globals(first).unwrap();
        assert_ne!(first, second);
        assert_eq!(ctx.module_globals(second).unwrap(), globals);
        assert_eq!(ctx.function_builtins(first).unwrap(), old);
        assert_eq!(ctx.function_builtins(second).unwrap(), updated);
        assert_eq!(ctx.new_function_globals(second).unwrap(), second);

        // sys.modules is mutable and cannot identify a function's original builtins.
        let original = ctx.module("builtins").unwrap();
        let alternate = ctx.module("sys").unwrap();
        let builtins_name = ctx.text("builtins");
        ctx.set_dictionary_key(ctx.modules, builtins_name, alternate, None, true)
            .unwrap();
        let ordinary = ctx.scope("app", None);
        ctx.bind(ordinary, "__builtins__", alternate);
        let captured = ctx.new_function_globals(ordinary).unwrap();
        assert_ne!(captured, ordinary);
        let expected = ctx.definition_builtins(alternate).unwrap();
        assert_eq!(ctx.function_builtins(captured).unwrap(), expected);
        ctx.set_dictionary_key(ctx.modules, builtins_name, original, None, true)
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_filename_labels_belong_to_the_interpreter() {
        let new_context = || {
            let argv = [c"imre".as_ptr()];
            // SAFETY: the constructor consumes the argument pointers before returning.
            unsafe { Box::from_raw(imre_dyn_new(1, argv.as_ptr(), c"{\"paths\":[]}".as_ptr())) }
        };
        let label = |ctx: &mut Context, filename: &'static CStr| {
            // SAFETY: the string literal has the same lifetime as an emitted filename.
            unsafe { ctx.native_filename_label(filename.as_ptr()) }.unwrap()
        };
        let mut first = new_context();
        let mut second = new_context();
        let raw = c"ledger-\xff.py";
        let first_label = Arc::downgrade(&label(&mut first, raw));
        let second_label = label(&mut second, raw);
        assert_eq!(&*label(&mut first, c"other.py"), "other.py");

        first.collect_generation(2).unwrap();
        let retained = first_label.upgrade().unwrap();
        assert!(Arc::ptr_eq(&retained, &label(&mut first, raw)));
        assert!(!Arc::ptr_eq(&retained, &second_label));
        drop(retained);
        drop(first);
        assert!(first_label.upgrade().is_none());
        assert!(Arc::ptr_eq(&second_label, &label(&mut second, raw)));
    }

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
        let value: Rc<[Vec<Id>]> = Rc::from([]);
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
        let value: Rc<[Vec<Id>]> = Rc::from([]);
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

        fn payload(ctx: &mut Context) -> (Id, Weak<[Vec<Id>]>) {
            let value: Rc<[Vec<Id>]> = Rc::from([]);
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
            .extend(["kept".into(), "other".into(), "unused".into()]);
        frame.closure.declared_complete = true;
        let (kept, kept_probe) = payload(&mut ctx);
        let (unused, unused_probe) = payload(&mut ctx);
        ctx.bind(outer, "kept", kept);
        ctx.bind(outer, "other", NONE);
        ctx.bind(outer, "unused", unused);
        let name = ctx.text("read");
        let signature = ctx.text(r#"{"scope_complete":true,"scope_reads":["kept","other"]}"#);
        let location = value::Location {
            file: "cells.py".into(),
            line: 1,
            span: None,
        };
        let filename = ctx.text(location.file.as_ref());
        let code = ctx.prelinked_code(
            value::Body::Native(read, NONE),
            outer,
            name,
            signature,
            (name, NONE),
            (filename, location.line),
        );
        let defaults = ctx.dict(Vec::new());
        let function = ctx
            .define_function(code, outer, name, defaults, name, NONE, NONE, false)
            .unwrap();
        let Value::Function { scope, closure, .. } = *ctx.heap.get(function) else {
            unreachable!()
        };
        assert_eq!(closure, NONE);
        let cell = ctx.scope_data(scope).closure.cells["kept"];
        let other = ctx.scope_data(scope).closure.cells["other"];
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
            if roots == [function] {
                let closure = ctx.lookup_attr(function, "__closure__").unwrap();
                assert_eq!(ctx.tuple_elements(closure), Some(&[cell, other][..]));
                assert_eq!(ctx.lookup_attr(function, "__closure__").unwrap(), closure);
            }
        }
    }
}
