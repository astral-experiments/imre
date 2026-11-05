use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::boolean::{Context as BooleanContext, Flow, Target};
use super::{Body, CallArguments, DefinitionName, Function, permanent};

enum Element<'a> {
    Value(&'a ast::Expr),
    Pair(&'a ast::Expr, &'a ast::Expr),
}

impl Function<'_, '_> {
    pub(super) fn expr(&mut self, expr: &ast::Expr) -> String {
        let position = imre_ast::expression_position(
            expr,
            &self.source.method_imports,
            Some(&self.source.private_names),
        );
        let line = self.source.line(position);
        let previous = self.expression_line;
        if previous == line {
            return self.expr_inner(expr, None);
        }
        self.expression_line = line;
        self.line(&format!(
            "call void @imre_dyn_expression_line(ptr %ctx, i32 {line})"
        ));
        let value = self.expr_inner(expr, None);
        self.expression_line = previous;
        if !self.terminated {
            self.line(&format!(
                "call void @imre_dyn_expression_line(ptr %ctx, i32 {previous})"
            ));
        }
        value
    }

    /// Select one field from a single-line partition call so both operations retain
    /// their expression line. Other receivers still dispatch through their methods.
    fn partition_index(&self, subscript: &ast::ExprSubscript) -> Option<u32> {
        let ast::Expr::Call(call) = &*subscript.value else {
            return None;
        };
        if self.source.line(subscript.start()) != self.source.line(subscript.end()) {
            return None;
        }
        let field = imre_ast::direct_method(
            call,
            &self.source.method_imports,
            Some(&self.source.private_names),
        )?;
        if !matches!(field.attr.as_str(), "partition" | "rpartition")
            || !call.arguments.keywords.is_empty()
            || !matches!(call.arguments.args.as_ref(), [arg] if !matches!(arg, ast::Expr::Starred(_)))
        {
            return None;
        }
        let ast::Expr::NumberLiteral(literal) = &*subscript.slice else {
            return None;
        };
        let ast::Number::Int(index) = &literal.value else {
            return None;
        };
        let index = u32::try_from(index.as_i64()?).ok()?;
        (index < 3).then_some(index)
    }

    fn expr_inner(&mut self, expr: &ast::Expr, partition: Option<(u32, String)>) -> String {
        match expr {
            ast::Expr::NoneLiteral(_) => permanent::NONE.to_string(),
            ast::Expr::EllipsisLiteral(_) => permanent::ELLIPSIS.to_string(),
            ast::Expr::BooleanLiteral(v) => (if v.value {
                permanent::TRUE
            } else {
                permanent::FALSE
            })
            .to_string(),
            ast::Expr::NumberLiteral(v) => match &v.value {
                ast::Number::Int(n) => match n.as_i64() {
                    Some(value) => self.integer(value),
                    None => self.lit(0, n.to_string().as_bytes()),
                },
                ast::Number::Float(value) => self.lit(3, &value.to_le_bytes()),
                ast::Number::Complex { real, imag } => {
                    self.lit(4, &[real.to_le_bytes(), imag.to_le_bytes()].concat())
                }
            },
            ast::Expr::StringLiteral(v) => self.literal_text(v.value.to_str(), v.value.iter()),
            ast::Expr::BytesLiteral(v) => self.lit(2, &v.value.bytes().collect::<Vec<_>>()),
            ast::Expr::Name(v) => self.lookup(v.id.as_str()),
            ast::Expr::Attribute(v) => {
                let base = self.expr(&v.value);
                self.attr(&base, v.attr.as_str())
            }
            ast::Expr::Subscript(v) => {
                if let Some(index) = self.partition_index(v) {
                    let span = self.expression_span(expr, &v.value, b'[');
                    let position =
                        span.map_or_else(|| "null".into(), |span| self.output.position(span));
                    return self.expr_inner(&v.value, Some((index, position)));
                }
                let object = self.expr(&v.value);
                let key = self.expr(&v.slice);
                let span = self.expression_span(expr, &v.value, b'[');
                self.op_at("get", &[&object, &key], span)
            }
            ast::Expr::Slice(slice) => {
                let [start, stop, step] = [&slice.lower, &slice.upper, &slice.step]
                    .map(|e| e.as_deref().map_or_else(|| "0".into(), |e| self.expr(e)));
                self.op("slice_object", &[&start, &stop, &step])
            }
            ast::Expr::List(v) => self
                .permanent_sequence(&v.elts, false)
                .unwrap_or_else(|| self.display("list", &v.elts)),
            ast::Expr::Set(v) => self.display("set", &v.elts),
            ast::Expr::Tuple(v) => {
                if let Some(tuple) = self.permanent_sequence(&v.elts, true) {
                    return tuple;
                }
                if let Some(tuple) = self.literal_string_tuple(&v.elts) {
                    return tuple;
                }
                let list = self.display("list", &v.elts);
                let op = if v.elts.iter().all(imre_ast::literal_tuple_element) {
                    "constant_tuple"
                } else {
                    "tuple"
                };
                self.op(op, &[&list])
            }
            ast::Expr::Dict(v) => {
                let dict = self.op("dict", &[]);
                for item in &v.items {
                    if let Some(key) = &item.key {
                        let key = self.expr(key);
                        let value = self.expr(&item.value);
                        self.op("put", &[&dict, &key, &value]);
                    } else {
                        let value = self.expr(&item.value);
                        self.op("update", &[&dict, &value]);
                    }
                }
                dict
            }
            ast::Expr::BinOp(v) => {
                let left = self.expr(&v.left);
                let right = self.expr(&v.right);
                self.op(v.op.as_str(), &[&left, &right])
            }
            ast::Expr::UnaryOp(v) => {
                let mark = (v.op == ast::UnaryOp::Not).then(|| self.temporary_mark());
                let operand = self.expr(&v.operand);
                let result = self.op(
                    match v.op {
                        ast::UnaryOp::Invert => "~",
                        ast::UnaryOp::Not => "not",
                        ast::UnaryOp::UAdd => "pos",
                        ast::UnaryOp::USub => "neg",
                    },
                    &[&operand],
                );
                if let Some(mark) = mark {
                    self.discard_truth(&mark);
                }
                result
            }
            ast::Expr::BoolOp(_) => self.boolean_value(expr),
            ast::Expr::Compare(v) => self.compare_expr(v, BooleanContext::Value),
            ast::Expr::If(v) => self.select(&v.test, |f| f.expr(&v.body), |f| f.expr(&v.orelse)),
            ast::Expr::Named(v) => {
                let ast::Expr::Name(target) = &*v.target else {
                    return self.unsupported("assignment expression target");
                };
                let mark = self.temporary_mark();
                let value = self.expr(&v.value);
                let name = self.code_name(target.id.as_str());
                let replaced =
                    self.op("assign_named_target", &[&self.scope.clone(), &name, &value]);
                self.line(&format!("call void @imre_dyn_named_assignment(ptr %ctx, i64 {mark}, i64 {value}, i64 {replaced})"));
                value
            }
            ast::Expr::Call(v) => {
                let mark = self.temporary_mark();
                // Cap the fixed entry-block scratch space even for calls in cold branches.
                let direct = v.arguments.args.len() <= 64
                    && v.arguments.keywords.is_empty()
                    && !v
                        .arguments
                        .args
                        .iter()
                        .any(|value| matches!(value, ast::Expr::Starred(_)));
                let single_spread = match v.arguments.args.as_ref() {
                    [ast::Expr::Starred(star)] => Some(star),
                    _ => None,
                };
                let packed = (!direct && single_spread.is_none()).then(|| self.op("list", &[]));
                let mut bound_receiver = None;
                let callable = if let Some(field) = imre_ast::direct_method(
                    v,
                    &self.source.method_imports,
                    Some(&self.source.private_names),
                ) {
                    let receiver = self.expr(&field.value);
                    let name = field.attr.as_str();
                    let lookup = if direct && name == "get" {
                        Some(format!(
                            "call i64 @imre_dyn_dict_get_target(ptr %ctx, i64 {receiver}, ptr %live)"
                        ))
                    } else if direct
                        && !name.starts_with('_')
                        && imre_ast::strings::STR_METHODS.contains(&name)
                    {
                        let symbol = self.output.data(format!("str.{name}").as_bytes());
                        Some(format!(
                            "call i64 @imre_dyn_string_method_target(ptr %ctx, i64 {receiver}, ptr {symbol}, ptr %live)"
                        ))
                    } else if direct && !name.starts_with('_') {
                        let symbol = self.output.data(name.as_bytes());
                        Some(format!(
                            "call i64 @imre_dyn_builtin_method_target(ptr %ctx, i64 {receiver}, ptr {symbol}, ptr %live)"
                        ))
                    } else {
                        None
                    };
                    if let Some(lookup) = lookup {
                        self.output.interned_identifiers.insert(name.to_owned());
                        self.max_live = self.max_live.max(1);
                        let target = self.checked(&lookup);
                        // Nested calls can reuse the entry-block scratch space.
                        bound_receiver = Some(self.value("load i64, ptr %live"));
                        target
                    } else {
                        self.named_op("method", &receiver, field.attr.as_str(), "0")
                    }
                } else {
                    self.expr(&v.func)
                };
                let operands = if let Some(star) = single_spread {
                    let source = self.expr(&star.value);
                    let kwargs = self.keyword_arguments(&v.arguments, &callable);
                    let args = self.op("spread_arguments", &[&source]);
                    CallArguments::Packed { args, kwargs }
                } else if let Some(args) = packed {
                    let kwargs = self.arguments(&args, &v.arguments, &callable);
                    CallArguments::Packed { args, kwargs }
                } else {
                    let values: Vec<_> = bound_receiver
                        .iter()
                        .cloned()
                        .chain(v.arguments.args.iter().map(|arg| self.expr(arg)))
                        .collect();
                    let (array, count) = self.handle_array(&values);
                    if let Some((index, get_position)) = partition {
                        debug_assert!(bound_receiver.is_some() && count == 2);
                        CallArguments::PartitionField {
                            array,
                            index,
                            get_position,
                        }
                    } else {
                        CallArguments::Positional {
                            array,
                            count,
                            receiver_slot: bound_receiver.is_some(),
                        }
                    }
                };
                let span = self.expression_span(expr, &v.func, b'(');
                // A caller inspecting this live frame needs the full call range, including
                // simple and multiline calls for which traceback highlighting is suppressed.
                let (start_line, start_col) = self.source.line_and_column(v.start());
                let (end_line, end_col) = self.source.line_and_column(v.end());
                let position = self
                    .output
                    .position([start_line, end_line, start_col, end_col]);
                self.line(&format!(
                    "call void @imre_dyn_call_position(ptr %ctx, ptr {position})"
                ));
                self.call_at(&callable, operands, &mark, span)
            }
            ast::Expr::Lambda(v) => self.define(
                v.start(),
                DefinitionName::Plain("<lambda>"),
                v.parameters.as_deref(),
                None,
                None,
                match v.body.as_ref() {
                    ast::Expr::Yield(_) => Body::YieldingLambda(&v.body),
                    body => Body::Expression(body),
                },
            ),
            ast::Expr::TString(v) => self.template(&v.value),
            ast::Expr::FString(v) => {
                let mut parts = Vec::new();
                for part in v.value.iter() {
                    match part {
                        ast::FStringPartRef::Literal(lit) => {
                            if !lit.value.is_empty() {
                                parts.push(self.literal_text(&lit.value, std::iter::once(lit)));
                            }
                        }
                        ast::FStringPartRef::FString(s) => {
                            self.format_parts(&s.elements, &mut parts)
                        }
                    }
                }
                self.join_format_parts(parts)
            }
            ast::Expr::Generator(v) => {
                if v.generators.iter().any(|generator| generator.is_async) {
                    return self.unsupported("async generator expression");
                }
                let iterable = self.expr(&v.generators[0].iter);
                let iter = self.op("iter", &[&iterable]);
                let step = self.define(
                    v.start(),
                    DefinitionName::Plain("<genexpr>"),
                    None,
                    None,
                    None,
                    Body::Generator(&v.generators, &v.elt),
                );
                let targets = self.comprehension_targets(&v.generators);
                self.op("generator", &[&step, &iter, &targets])
            }
            ast::Expr::ListComp(v) => {
                self.comprehension("list", &v.generators, Element::Value(&v.elt))
            }
            ast::Expr::SetComp(v) => {
                self.comprehension("set", &v.generators, Element::Value(&v.elt))
            }
            ast::Expr::DictComp(v) => match &v.key {
                Some(key) => {
                    self.comprehension("dict", &v.generators, Element::Pair(key, &v.value))
                }
                None => self.unsupported("dict unpacking comprehension"),
            },
            _ => self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(expr).kind())),
        }
    }

    /// Jump to the selected suite without materializing or retesting a short-circuit operand.
    pub(super) fn branch_expr(&mut self, expr: &ast::Expr, yes: &str, no: &str) {
        let test = |f: &mut Self, expr: &ast::Expr, yes: &str, no: &str| {
            let condition = f.test_operand(expr);
            f.branch(&condition, yes, no);
        };
        if BooleanContext::Condition.splits(expr) {
            self.boolean_steps(expr, BooleanContext::Condition, yes, no, test);
        } else {
            test(self, expr, yes, no);
        }
    }

    fn test_operand(&mut self, expr: &ast::Expr) -> String {
        let mark = self.temporary_mark();
        let condition = if let ast::Expr::Compare(comparison) = expr {
            self.compare_expr(comparison, BooleanContext::Condition)
        } else {
            let value = self.expr(expr);
            self.truth(&value)
        };
        self.discard_truth(&mark);
        condition
    }

    /// Assign a block to each graph step. Multiple predecessors can share a single operand.
    fn boolean_steps(
        &mut self,
        expr: &ast::Expr,
        context: BooleanContext,
        yes: &str,
        no: &str,
        mut emit: impl FnMut(&mut Self, &ast::Expr, &str, &str),
    ) {
        let flow = Flow::new(expr, context);
        let labels: Vec<_> = flow.steps.iter().map(|_| self.name("b")).collect();
        let label = |target| match target {
            Target::Step(i) => labels[i].as_str(),
            Target::Truth(true) => yes,
            Target::Truth(false) => no,
            Target::Operand => {
                debug_assert_eq!(yes, no);
                yes
            }
        };
        for (i, step) in flow.steps.iter().enumerate().rev() {
            if i + 1 != flow.steps.len() {
                self.label(&labels[i]);
            }
            emit(self, step.expr, label(step.yes), label(step.no));
        }
    }

    /// Keep the selected object at the join. Discard other operand roots before visiting the next.
    fn boolean_value(&mut self, expr: &ast::Expr) -> String {
        let done = self.name("b");
        let mut incoming = Vec::new();
        self.boolean_steps(
            expr,
            BooleanContext::Value,
            &done,
            &done,
            |f, expr, yes, no| {
                if yes == no {
                    let value = f.expr(expr);
                    incoming.push(format!("[ {value}, %{} ]", f.current_block()));
                    f.jump(&done);
                    return;
                }
                let mark = f.temporary_mark();
                let value = f.expr(expr);
                let truth = f.truth(&value);
                if yes != done && no != done {
                    f.discard_truth(&mark);
                    f.branch(&truth, yes, no);
                    return;
                }
                incoming.push(format!("[ {value}, %{} ]", f.current_block()));
                let discard = f.name("b");
                let next = if yes == done {
                    f.branch(&truth, &done, &discard);
                    no
                } else {
                    f.branch(&truth, &discard, &done);
                    yes
                };
                f.label(&discard);
                f.discard_truth(&mark);
                f.jump(next);
            },
        );
        self.label(&done);
        self.value(&format!("phi i64 {}", incoming.join(", ")))
    }

    /// A failed intermediate comparison returns its object in a value context and false in a branch.
    fn compare_expr(&mut self, comparison: &ast::ExprCompare, context: BooleanContext) -> String {
        let mut left = self.expr(&comparison.operands[0]);
        if let [op @ (ast::CmpOp::In | ast::CmpOp::NotIn)] = comparison.ops.as_ref()
            && let ast::Expr::Tuple(tuple) = &comparison.operands[1]
            && tuple.elts.len() <= 8
            && tuple.elts.iter().all(imre_ast::literal_tuple_element)
        {
            // Keep the entry-block scratch array bounded even for very large tuple displays.
            let values: Vec<_> = tuple.elts.iter().map(|item| self.expr(item)).collect();
            let (array, count) = self.handle_array(&values);
            let negate = u32::from(*op == ast::CmpOp::NotIn);
            let value = self.checked(&format!(
                "call i64 @imre_dyn_contains_literal_tuple(ptr %ctx, i64 {left}, ptr {array}, i64 {count}, i32 {negate})"
            ));
            return if context == BooleanContext::Condition {
                self.truth(&value)
            } else {
                value
            };
        }
        let done = self.name("b");
        let mut incoming = Vec::new();
        for (i, (op, operand)) in comparison
            .ops
            .iter()
            .zip(&comparison.operands[1..])
            .enumerate()
        {
            let right = self.expr(operand);
            let value = self.op(op.as_str(), &[&left, &right]);
            left = right;
            if i + 1 == comparison.ops.len() {
                let result = if context == BooleanContext::Condition {
                    self.truth(&value)
                } else {
                    value
                };
                if incoming.is_empty() {
                    return result;
                }
                incoming.push(format!("[ {result}, %{} ]", self.current_block()));
                self.jump(&done);
            } else {
                let truth = self.truth(&value);
                let result = if context == BooleanContext::Condition {
                    "false"
                } else {
                    &value
                };
                incoming.push(format!("[ {result}, %{} ]", self.current_block()));
                let next = self.name("b");
                self.branch(&truth, &next, &done);
                self.label(&next);
            }
        }
        self.label(&done);
        let ty = if context == BooleanContext::Condition {
            "i1"
        } else {
            "i64"
        };
        self.value(&format!("phi {ty} {}", incoming.join(", ")))
    }

    /// Put immutable element handles in the module data for one list or tuple construction.
    fn permanent_sequence(&mut self, elements: &[ast::Expr], tuple: bool) -> Option<String> {
        if elements.is_empty() {
            return None;
        }
        let mut bytes = Vec::new();
        for element in elements {
            let id = match element {
                ast::Expr::NoneLiteral(_) => permanent::NONE,
                ast::Expr::EllipsisLiteral(_) => permanent::ELLIPSIS,
                ast::Expr::BooleanLiteral(v) => {
                    if v.value {
                        permanent::TRUE
                    } else {
                        permanent::FALSE
                    }
                }
                ast::Expr::NumberLiteral(v) => {
                    let ast::Number::Int(n) = &v.value else {
                        return None;
                    };
                    permanent::small_integer_id(n.as_i64()?)?
                }
                _ => return None,
            };
            bytes.extend_from_slice(&u16::try_from(id).ok()?.to_le_bytes());
        }
        let data = self.output.data(&bytes);
        Some(self.checked(&format!(
            "call i64 @imre_dyn_permanent_sequence(ptr %ctx, ptr {data}, i64 {}, i1 {tuple})",
            bytes.len()
        )))
    }

    fn display(&mut self, kind: &str, elements: &[ast::Expr]) -> String {
        let target = self.op(kind, &[]);
        for element in elements {
            if let ast::Expr::Starred(v) = element {
                let items = self.expr(&v.value);
                self.op("extend", &[&target, &items]);
            } else {
                let value = self.expr(element);
                self.op("append", &[&target, &value]);
            }
        }
        target
    }

    pub(super) fn arguments(
        &mut self,
        args: &str,
        syntax: &ast::Arguments,
        callable: &str,
    ) -> String {
        // Python evaluates starred positional arguments before the keyword arguments.
        for arg in &syntax.args {
            if let ast::Expr::Starred(s) = arg {
                let value = self.expr(&s.value);
                self.op("extend", &[args, &value]);
            } else {
                let value = self.expr(arg);
                self.op("append", &[args, &value]);
            }
        }
        self.keyword_arguments(syntax, callable)
    }

    /// Evaluate keyword expressions and merge mappings in source order.
    fn keyword_arguments(&mut self, syntax: &ast::Arguments, callable: &str) -> String {
        let kwargs = if syntax.keywords.is_empty() {
            "0".into()
        } else {
            self.op("dict", &[])
        };
        let mut keywords = syntax.keywords.iter().peekable();
        let mut unpacked = false;
        while let Some(key) = keywords.peek() {
            if key.arg.is_none() {
                let value = self.expr(&keywords.next().unwrap().value);
                self.op("keywords", &[&kwargs, &value, callable]);
                unpacked = true;
            } else {
                // Python builds each group of named arguments before merging it
                // with earlier **mappings. Even a duplicate evaluates the group.
                let group = if unpacked {
                    self.op("dict", &[])
                } else {
                    kwargs.clone()
                };
                while let Some(key) = keywords.next_if(|key| key.arg.is_some()) {
                    let value = self.expr(&key.value);
                    self.named_op(
                        "keyword",
                        &group,
                        key.arg.as_ref().unwrap().as_str(),
                        &value,
                    );
                }
                if unpacked {
                    self.op("keywords", &[&kwargs, &group, callable]);
                }
            }
        }
        kwargs
    }

    /// Capture values once, leaving conversion and formatting to the template consumer.
    fn template(&mut self, template: &ast::TStringValue) -> String {
        let strings = self.template_strings(template);
        let parts = self.op("list", &[]);
        for field in template.iter().flat_map(|t| t.elements.interpolations()) {
            let spec = match imre_ast::strings::template_format_spec(field) {
                Ok(Some(lit)) => self.format_literal(lit),
                Ok(None) => self.text(""),
                Err(reason) => return self.unsupported(reason),
            };
            let Some(spelling) = self.source.template_expressions.get(&field.start()) else {
                return self.unsupported("t-string expression spelling");
            };
            let expression = self.text(spelling);
            let value = self.expr(&field.expression);
            let conversion = field
                .conversion
                .to_char()
                .map_or_else(|| "0".to_owned(), |c| self.text(&c.to_string()));
            let metadata = self.op("list", &[]);
            self.op("append", &[&metadata, &conversion]);
            self.op("append", &[&metadata, &spec]);
            let part = self.op("template_interpolation", &[&value, &expression, &metadata]);
            self.op("append", &[&parts, &part]);
        }
        self.op("build_template", &[&strings, &parts])
    }

    /// Evaluate the fields and their conversions in source order before joining their text.
    fn format_parts(
        &mut self,
        elements: &[ast::InterpolatedStringElement],
        parts: &mut Vec<String>,
    ) {
        for element in elements {
            let value = match element {
                ast::InterpolatedStringElement::Literal(lit) if lit.value.is_empty() => continue,
                ast::InterpolatedStringElement::Literal(lit) => self.format_literal(lit),
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(debug) = &field.debug_text {
                        parts.push(self.text(debug.as_str()));
                    }
                    let mut value = self.expr(&field.expression);
                    let conversion = field.conversion.to_char().or_else(|| {
                        (field.debug_text.is_some() && field.format_spec.is_none()).then_some('r')
                    });
                    if let Some(c) = conversion {
                        let conversion = self.text(&c.to_string());
                        value = self.op("format_conversion", &[&value, &conversion]);
                    }
                    let spec = match &field.format_spec {
                        Some(s) => {
                            let mut spec = Vec::new();
                            self.format_parts(&s.elements, &mut spec);
                            self.join_format_parts(spec)
                        }
                        None => self.text(""),
                    };
                    self.op("format", &[&value, "0", &spec])
                }
            };
            parts.push(value);
        }
    }

    /// Preserve a lone formatted value and bypass string-subclass addition when joining fields.
    fn join_format_parts(&mut self, parts: Vec<String>) -> String {
        parts
            .into_iter()
            .reduce(|left, right| self.op("format_concat", &[&left, &right]))
            .unwrap_or_else(|| self.text(""))
    }

    /// Resume the innermost active loop, returning to its parent when it runs out of items.
    pub(super) fn generator_step(
        &mut self,
        generators: &[ast::Comprehension],
        element: &ast::Expr,
    ) {
        let outer = self.lookup(".iterator");
        let loops: Vec<_> = (0..generators.len())
            .map(|i| (self.name("b"), format!(".iterator.{i}")))
            .collect();
        let done = self.name("b");
        if loops.len() > 1 {
            // Only the initial call has the outer iterator in .iterator. Every yield is
            // inside the last clause, with each active iterator saved in the frame.
            let first = self.name("b");
            let resumed = self.value(&format!("icmp eq i64 {outer}, 0"));
            self.branch(&resumed, &loops.last().unwrap().0, &first);
            self.label(&first);
            self.bind(&loops[0].1, &outer);
            self.bind(".iterator", "0");
        }
        self.jump(&loops[0].0);
        for (i, generator) in generators.iter().enumerate() {
            let (step, slot) = &loops[i];
            let body = self.name("b");
            let exhausted = if i == 0 { done.clone() } else { self.name("b") };
            self.label(step);
            let iter = if loops.len() == 1 {
                outer.clone()
            } else {
                self.lookup(slot)
            };
            let more = self.op("more", &[&iter]);
            let cond = self.value(&format!("icmp eq i64 {more}, 2"));
            self.branch(&cond, &body, &exhausted);
            self.label(&body);
            self.checkpoint(element);
            let item = self.op("next", &[&iter]);
            self.assign(&generator.target, &item);
            for filter in &generator.ifs {
                let keep = self.name("b");
                self.branch_expr(filter, &keep, step);
                self.label(&keep);
            }
            if let Some(child) = generators.get(i + 1) {
                let iterable = self.expr(&child.iter);
                let iterator = self.op("iter", &[&iterable]);
                self.bind(&loops[i + 1].1, &iterator);
                self.jump(&loops[i + 1].0);
            } else {
                let item = self.expr(element);
                let yielded = self.op("yielded", &[&item]);
                self.end(&format!("ret i64 {yielded}"));
            }
            if i > 0 {
                self.label(&exhausted);
                self.bind(slot, "0");
                self.jump(&loops[i - 1].0);
            }
        }
        self.label(&done);
    }

    /// Encode all loop bindings before any comprehension iteration begins.
    fn comprehension_targets(&mut self, generators: &[ast::Comprehension]) -> String {
        let mut names = std::collections::HashSet::new();
        for clause in generators {
            crate::frontend::target_names(&clause.target, &mut names);
        }
        let mut names: Vec<_> = names.into_iter().collect();
        names.sort_unstable();
        self.text(&names.join(" "))
    }

    fn comprehension(
        &mut self,
        kind: &str,
        generators: &[ast::Comprehension],
        element: Element<'_>,
    ) -> String {
        if generators.iter().any(|g| g.is_async) {
            return self.unsupported("async comprehension");
        }
        let output = self.op(kind, &[]);
        let first = self.expr(&generators[0].iter);
        let targets = self.comprehension_targets(generators);
        let scope = self.op("comprehension_scope", &[&self.scope.clone(), &targets]);
        let previous = std::mem::replace(&mut self.scope, scope);
        let class_locals = self.class_locals.take();
        self.comprehension_loop(generators, &first, &output, &element);
        self.class_locals = class_locals;
        self.scope = previous;
        output
    }

    fn comprehension_loop(
        &mut self,
        generators: &[ast::Comprehension],
        source: &str,
        out: &str,
        element: &Element<'_>,
    ) {
        let g = &generators[0];
        let iter = self.op("iter", &[source]);
        let mark = self.temporary_mark();
        let step = self.name("b");
        let body = self.name("b");
        let done = self.name("b");
        self.jump(&step);
        self.label(&step);
        self.line(&format!(
            "call void @imre_dyn_comprehension_step(ptr %ctx, i64 {mark})"
        ));
        let more = self.op("more", &[&iter]);
        let cond = self.value(&format!("icmp eq i64 {more}, 2"));
        self.branch(&cond, &body, &done);
        self.label(&body);
        let item = self.op("next", &[&iter]);
        self.assign(&g.target, &item);
        for test in &g.ifs {
            let keep = self.name("b");
            self.branch_expr(test, &keep, &step);
            self.label(&keep);
        }
        if let Some(next) = generators.get(1) {
            let source = self.expr(&next.iter);
            self.comprehension_loop(&generators[1..], &source, out, element);
        } else {
            match element {
                Element::Value(e) => {
                    let v = self.expr(e);
                    self.op("append", &[out, &v]);
                }
                Element::Pair(k, v) => {
                    let k = self.expr(k);
                    let v = self.expr(v);
                    self.op("put", &[out, &k, &v]);
                }
            }
        }
        self.jump(&step);
        self.label(&done);
    }
}
