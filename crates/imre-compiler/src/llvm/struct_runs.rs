//! Fold uniform runs and repeated cycles of pure scalar-row projections when binary64 addition is exact.
use super::*;
use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;

struct RunSum<'a> {
    list: usize,
    cursor: usize,
    row: usize,
    accumulator: usize,
    term: &'a Expr,
}

/// Restrict the projection to total floating-point arithmetic and immutable fields.
/// A direct single-argument method may contain the same arithmetic in its return.
fn pure_term(
    module: Option<&Module>,
    value: &Expr,
    row: &dyn Fn(&Expr) -> bool,
    accumulator: Option<usize>,
) -> bool {
    if value.ty != Type::Float {
        return false;
    }
    match &value.kind {
        ExprKind::Float(_) => true,
        ExprKind::Local(slot) => accumulator.is_some_and(|sum| *slot != sum),
        ExprKind::Field(receiver, _) => row(receiver),
        ExprKind::Unary(UnaryOp::USub, inner) => pure_term(module, inner, row, accumulator),
        ExprKind::Binary(Binary::Add | Binary::Subtract | Binary::Multiply, left, right) => {
            pure_term(module, left, row, accumulator) && pure_term(module, right, row, accumulator)
        }
        ExprKind::Call(id, args) => {
            let Some(module) = module else { return false };
            let f = &module.functions[*id];
            matches!((args.values.as_slice(), f.params.as_slice(), f.body.as_slice()),
                ([arg], [receiver], [Statement::Return(result)])
                if row(arg) && (matches!(arg.kind, ExprKind::Local(_)) || structs::field_reader(f)) && pure_term(None, result, &|v| matches!(v.kind, ExprKind::Local(slot) if slot == *receiver), None))
        }
        _ => false,
    }
}

/// Match an actual list traversal whose only observable body update is a floating sum.
fn run_sum<'a>(module: &Module, test: &Expr, body: &'a [Statement]) -> Option<RunSum<'a>> {
    let [
        Statement::Assign(row, item),
        Statement::Assign(cursor, advance),
        Statement::Assign(sum, update),
    ] = body
    else {
        return None;
    };
    let ExprKind::Builtin(Builtin::SequenceGetInBounds, args) = &item.kind else {
        return None;
    };
    let [receiver, index] = args.as_slice() else {
        return None;
    };
    let ExprKind::Local(list) = receiver.kind else {
        return None;
    };
    let local =
        |expr: &Expr, expected| matches!(expr.kind, ExprKind::Local(slot) if slot == expected);
    let ExprKind::Compare(parts, ops) = &test.kind else {
        return None;
    };
    let ([start, bound], [CmpOp::Lt]) = (parts.as_slice(), ops.as_slice()) else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, old_index, one) = &advance.kind else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, old_sum, term) = &update.kind else {
        return None;
    };
    (module.types.is_value_struct(item.ty)
        && local(index, *cursor) && local(start, *cursor) && local(old_index, *cursor)
        && matches!(&bound.kind, ExprKind::Builtin(Builtin::Len, a) if matches!(a.as_slice(), [v] if local(v, list)))
        && matches!(&one.kind, ExprKind::Int(n) if n == "1")
        && local(old_sum, *sum) && old_sum.ty == Type::Float
        && pure_term(Some(module), term, &|v| local(v, *row), Some(*sum)))
    .then_some(RunSum { list, cursor: *cursor, row: *row, accumulator: *sum, term })
}

/// Match a projection from a single list at the current modular index.
fn indexed_list(module: &Module, value: &Expr, index: usize) -> Option<usize> {
    let (_, [receiver, subscript]) = structs::packed_get(value)? else {
        return None;
    };
    let ExprKind::Local(list) = receiver.kind else {
        return None;
    };
    (module.types.is_value_struct(value.ty)
        && matches!(subscript.kind, ExprKind::Local(slot) if slot == index))
    .then_some(list)
}

/// Carry the established modular bounds into each row projection in this pure expression.
fn mark_indexed_reads_in_bounds(module: &Module, term: &mut Expr, list: usize, index: usize) {
    if indexed_list(module, term, index) == Some(list)
        && let ExprKind::Builtin(op, _) = &mut term.kind
    {
        *op = Builtin::SequenceGetInBounds;
    } else {
        ir::children_mut(term, |child| {
            mark_indexed_reads_in_bounds(module, child, list, index)
        });
    }
}

impl Emitter<'_, '_> {
    /// Evaluate a validated, total float projection on one borrowed dictionary payload.
    /// Field-only methods can use the same projection on their receiver without boxing it.
    fn dictionary_projection(&mut self, term: &Expr, payload: &str) -> Value {
        match &term.kind {
            ExprKind::Float(_) | ExprKind::Local(_) => self.expr(term),
            ExprKind::Field(receiver, field) => {
                let row = Value {
                    ty: receiver.ty,
                    operand: payload.to_owned(),
                    small_integer: false,
                };
                let pointer = self.field_pointer(&row, *field);
                self.field_value(&pointer, term.ty)
            }
            ExprKind::Unary(UnaryOp::USub, inner) => {
                let value = self.dictionary_projection(inner, payload);
                Value {
                    operand: self.instruction(format!("fneg double {}", value.operand)),
                    ..value
                }
            }
            ExprKind::Binary(op, left, right) => {
                let left = self.dictionary_projection(left, payload);
                let right = self.dictionary_projection(right, payload);
                self.binary(*op, &left, &right)
            }
            ExprKind::Call(id, _) => {
                let [Statement::Return(result)] = self.module.functions[*id].body.as_slice() else {
                    unreachable!()
                };
                self.dictionary_projection(result, payload)
            }
            _ => unreachable!("validated dictionary projection"),
        }
    }

    /// Fold in-bounds modular lookups across runs of the same immutable row.
    /// Publish the final index and loop target of each bounded chunk before polling again.
    pub(super) fn uniform_indexed_sum(
        &mut self,
        body: &[Statement],
        target: usize,
        cursor: usize,
        args: &[Value],
        exhausted: &str,
    ) {
        let [
            Statement::Assign(index, subscript),
            Statement::Assign(sum, update),
        ] = body
        else {
            return;
        };
        let ExprKind::Binary(Binary::Add, old_sum, term) = &update.kind else {
            return;
        };
        let ExprKind::Binary(Binary::Modulo, numerator, divisor) = &subscript.kind else {
            return;
        };
        if self.locals[*sum] != Type::Float
            || self.locals[*index] != Type::Int
            || old_sum.ty != Type::Float
            || !matches!(old_sum.kind, ExprKind::Local(slot) if slot == *sum)
            || *index == target
            || !self.module.source_frames.is_empty()
        {
            return;
        }
        let Some((scale, offset, limit)) = affine::bounded_ordinal(target, numerator) else {
            return;
        };
        let mut list = None;
        ir::walk_expressions(body, &mut |e| {
            if list.is_none() {
                list = indexed_list(self.module, e, *index);
            }
        });
        let Some(list) = list else { return };
        if !matches!(self.locals[list], Type::List(_)) {
            return;
        }
        if !pure_term(
            Some(self.module),
            term,
            &|e| indexed_list(self.module, e, *index) == Some(list),
            Some(*sum),
        ) {
            return;
        }
        let modulus = match &divisor.kind {
            ExprKind::Local(slot)
                if self.locals[*slot] == Type::Int && *slot != target && *slot != *index =>
            {
                self.borrow_slot(*slot)
            }
            ExprKind::Int(n) => {
                let Ok(n) = n.parse::<i32>() else { return };
                small(i64::from(n))
            }
            _ => return,
        };
        let (start, stop, modulus) = (&args[0].operand, &args[1].operand, &modulus.operand);
        let ordinary = self.label();
        let inspect = self.label();
        let runs = self.entry_alloca("{ i64, [8 x i64] }");
        self.output
            .local_helper("imre_uniform_index_range", INDEX_RANGE_HELPER);
        let eligible = self.instruction(format!(
            "call i1 @imre_uniform_index_range(i64 {start}, i64 {stop}, i64 {modulus}, i64 {limit})"
        ));
        self.branch(&eligible, &inspect, &ordinary);
        self.start(&inspect);
        let receiver = self.borrow_slot(list);
        let present = self.instruction(format!("icmp ne ptr {}, null", receiver.operand));
        let bounds = self.label();
        self.branch(&present, &bounds, &ordinary);
        self.start(&bounds);
        let length = self.container_length(&receiver);
        let in_bounds = self.instruction(format!("icmp ule i64 {modulus}, {}", length.operand));
        let prepare = self.label();
        self.branch(&in_bounds, &prepare, &ordinary);
        self.start(&prepare);
        self.output
            .local_helper("imre_struct_modular_runs", include_str!("modular_rows.ll"));
        let distance = self.instruction(format!("sub i64 {stop}, {start}"));
        let count = self.instruction(format!(
            "call i64 @imre_struct_modular_runs(ptr {}, i64 {modulus}, i64 {scale}, i64 {offset}, i64 {distance}, ptr {runs})",
            receiver.operand
        ));
        let scalar = self.label();
        let planned = self.label();
        let unplanned = self.instruction(format!("icmp slt i64 {count}, 0"));
        if pure_term(
            Some(self.module),
            term,
            &|e| indexed_list(self.module, e, *index) == Some(list),
            None,
        ) {
            let (dictionary, project, publish) = (self.label(), self.label(), self.label());
            self.branch(&unplanned, &dictionary, &planned);
            self.start(&dictionary);
            let view = self.entry_alloca("{ [2 x ptr], ptr, i64 }");
            let alternate = self.instruction(format!(
                "call i64 @imre_indexed_dictionary(ptr {}, ptr {view}, i64 {start}, i64 {stop}, i64 {modulus}, i64 {scale}, i64 {offset})",
                receiver.operand
            ));
            let found = self.instruction(format!("icmp sge i64 {alternate}, 0"));
            self.branch(&found, &project, &scalar);
            self.start(&project);
            let terms = [0, 1].map(|i| {
                let entry = self.instruction(format!("getelementptr ptr, ptr {view}, i64 {i}"));
                let payload = self.instruction(format!("load ptr, ptr {entry}"));
                self.dictionary_projection(term, &payload).operand
            });
            self.output.local_helper(
                "imre_bitmap_indexed_sum",
                include_str!("bitmap_indexed_sum.ll"),
            );
            let initial = self.borrow_slot(*sum);
            let result = self.instruction(format!("call {{ double, i64 }} @imre_bitmap_indexed_sum(ptr {view}, i64 {start}, i64 {stop}, i64 {modulus}, i64 {}, i64 {alternate}, i64 {scale}, i64 {offset}, double {}, double {}, double {})", length.operand, initial.operand, terms[0], terms[1]));
            let end = self.instruction(format!("extractvalue {{ double, i64 }} {result}, 1"));
            let advanced = self.instruction(format!("icmp ne i64 {end}, {start}"));
            self.branch(&advanced, &publish, &scalar);
            self.start(&publish);
            let subtotal = self.instruction(format!("extractvalue {{ double, i64 }} {result}, 0"));
            self.line(format!("store double {subtotal}, ptr %local{sum}"));
            let last = self.instruction(format!("sub i64 {end}, 2"));
            let at = self.instruction(format!("call i64 @imre_bounded_modular_index(i64 {last}, i64 {scale}, i64 {offset}, i64 {modulus})"));
            for (slot, operand) in [(target, last), (*index, at), (cursor, end)] {
                self.store(
                    slot,
                    &Value {
                        ty: Type::Int,
                        operand,
                        small_integer: true,
                    },
                );
            }
            self.jump(&ordinary);
        } else {
            self.branch(&unplanned, &scalar, &planned);
        }
        for (entry, single_rows) in [(scalar, true), (planned, false)] {
            self.start(&entry);
            let condition = self.label();
            let batch = self.label();
            self.jump(&condition);
            self.start(&condition);
            // Relinquish the read-only proof before running any signal handler.
            // The ordinary range header polls with the fully published prefix.
            let tripped = self.instruction(
                "load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1",
            );
            let pending = self.instruction(format!("icmp ne i8 {tripped}, 0"));
            let test = self.label();
            self.branch(&pending, &ordinary, &test);
            self.start(&test);
            let at = self.borrow_slot(cursor);
            let more = self.instruction(format!("icmp slt i64 {}, {stop}", at.operand));
            self.branch(&more, &batch, exhausted);
            self.start(&batch);
            let (end, last, repetitions) = if single_rows {
                let end = self.instruction(format!("add i64 {}, 2", at.operand));
                (end, at.operand, "1".to_owned())
            } else {
                let left = self.instruction(format!("sub i64 {stop}, {}", at.operand));
                let span = self.instruction(format!(
                    "call i64 @imre_modular_batch_width(i64 {}, i64 {left}, i64 {count}, ptr {runs})",
                    at.operand
                ));
                let repetitions = self.instruction(format!("lshr i64 {span}, 1"));
                let end = self.instruction(format!("add i64 {}, {span}", at.operand));
                let last = self.instruction(format!("sub i64 {end}, 2"));
                (end, last, repetitions)
            };
            let subscript = self.instruction(format!(
                "call i64 @imre_bounded_modular_index(i64 {last}, i64 {scale}, i64 {offset}, i64 {modulus})"
            ));
            let ordinal = |operand| Value {
                ty: Type::Int,
                operand,
                small_integer: true,
            };
            self.store(target, &ordinal(last));
            self.store(*index, &ordinal(subscript));
            let mut projection = term.as_ref().clone();
            mark_indexed_reads_in_bounds(self.module, &mut projection, list, *index);
            let value = self.expr(&projection);
            let old = self.borrow_slot(*sum);
            self.add_repeated_term(*sum, &old, &value, &repetitions);
            self.store(cursor, &ordinal(end));
            self.jump(&condition);
        }
        self.start(&ordinary);
    }

    /// Advance over the current read-only run after copying its observable loop target.
    /// A dense or replaced receiver follows the ordinary traversal.
    pub(super) fn struct_run_loop(
        &mut self,
        test: &Expr,
        body: &[Statement],
        ordinary: &str,
        exhausted: &str,
    ) -> bool {
        let Some(plan) = run_sum(self.module, test, body) else {
            return false;
        };
        if !self.struct_locals[plan.row] || !self.module.source_frames.is_empty() {
            return false;
        }
        let Some(view) =
            self.length_roots[plan.list].and_then(|r| self.length_snapshots[r].clone())
        else {
            return false;
        };
        let Some(ListBuffer::Struct {
            data,
            index_mask,
            cursor,
        }) = view.data
        else {
            return false;
        };

        // Keep the original dense traversal intact. A packed repeated list
        // broadcasts its first row, while a null view uses the sparse cursor.
        let list = self.borrow_slot(plan.list);
        let same = self.instruction(format!(
            "icmp eq ptr {}, {}",
            list.operand, view.receiver.operand
        ));
        let sparse = self.instruction(format!("icmp eq ptr {data}, null"));
        let repeated = self.instruction(format!("icmp eq i64 {index_mask}, 0"));
        let compressed = self.instruction(format!("or i1 {sparse}, {repeated}"));
        let eligible = self.instruction(format!("and i1 {same}, {compressed}"));
        let (condition, visit) = (self.label(), self.label());
        let other = self.label();
        let shared = self.label();
        self.branch(&eligible, &shared, &other);
        self.start(&shared);
        let at = self.borrow_slot(plan.cursor);
        let first = self.instruction(format!("icmp eq i64 {}, 1", at.operand));
        let query = self.instruction(format!("and i1 {sparse}, {first}"));
        let binary = self.label();
        self.branch(&query, &binary, &condition);
        self.start(&binary);
        self.binary_struct_sum(
            &plan,
            &list.operand,
            &view.length.operand,
            &cursor,
            &condition,
            ordinary,
        );
        self.start(&other);
        // Uniform and sparse views have already dispatched above. Only a
        // matching periodic view can reuse a complete cycle of projections.
        let masked = self.instruction(format!("icmp ne i64 {index_mask}, -1"));
        let cyclic = self.instruction(format!("and i1 {same}, {masked}"));
        let cycle = self.label();
        self.branch(&cyclic, &cycle, ordinary);
        self.start(&cycle);
        self.periodic_struct_sum(&plan, body, &view.length.operand, &index_mask, ordinary);
        self.start(&condition);
        self.check_interrupt();
        CheckedBody::native(self).branch_condition(test, &visit, exhausted);
        self.start(&visit);

        self.block(&body[..2]);
        let next = self.borrow_slot(plan.cursor);
        let next = self.instruction(format!("lshr i64 {}, 1", next.operand));
        let at = self.instruction(format!("sub i64 {next}, 1"));
        let length = self.instruction(format!("lshr i64 {}, 1", view.length.operand));
        let sparse_end = self.instruction(format!(
            "call i64 @imre_struct_uniform_end(ptr {cursor}, i64 {next})"
        ));
        let end = self.instruction(format!(
            "select i1 {sparse}, i64 {sparse_end}, i64 {length}"
        ));
        let count = self.instruction(format!("sub i64 {end}, {at}"));

        let sum = self.borrow_slot(plan.accumulator);
        let term = self.expr(plan.term);
        // The exact bulk path is constant time. Bound only the scalar fallback
        // so a long uniform run returns regularly to a Python loop checkpoint.
        let exact = self.instruction(format!(
            "call i1 @imre_exact_fadd_bound(double {}, double {}, i64 {count})",
            sum.operand, term.operand
        ));
        let short = self.instruction(format!("icmp ule i64 {count}, 65536"));
        let whole = self.instruction(format!("or i1 {exact}, {short}"));
        let count = self.instruction(format!("select i1 {whole}, i64 {count}, i64 65536"));
        let end = self.instruction(format!("add i64 {at}, {count}"));
        self.add_repeated_term(plan.accumulator, &sum, &term, &count);
        let shifted = self.instruction(format!("shl i64 {end}, 1"));
        let tagged = self.instruction(format!("or i64 {shifted}, 1"));
        self.line(format!("store i64 {tagged}, ptr %local{}", plan.cursor));
        self.jump(&condition);
        true
    }

    /// Apply the projected term in source order unless binary64 reassociation is exact.
    fn add_repeated_term(&mut self, accumulator: usize, sum: &Value, term: &Value, count: &str) {
        self.output
            .local_helper("imre_repeat_fadd", STRUCT_RUN_HELPERS);
        let result = self.instruction(format!(
            "call double @imre_repeat_fadd(double {}, double {}, i64 {count})",
            sum.operand, term.operand
        ));
        self.line(format!("store double {result}, ptr %local{}", accumulator));
    }

    /// Evaluate one pure projection per dictionary entry. Keep the ordinary last
    /// iteration so the cursor and observable row target have their original values.
    fn binary_struct_sum(
        &mut self,
        plan: &RunSum<'_>,
        list: &str,
        length: &str,
        cursor: &str,
        ordered: &str,
        ordinary: &str,
    ) {
        self.check_interrupt();
        let rows = self.entry_alloca("{ [2 x ptr], ptr, i64 }");
        let count = self.instruction(format!("lshr i64 {length}, 1"));
        let prefix = self.instruction(format!("sub i64 {count}, 1"));
        let (bitmap, sparse, check) = (self.label(), self.label(), self.label());
        self.jump(&bitmap);
        self.start(&bitmap);
        let bitmap_count = self.instruction(format!(
            "call i64 @imre_struct_binary_prefix(ptr {list}, ptr {rows})"
        ));
        let has_bitmap = self.instruction(format!("icmp sge i64 {bitmap_count}, 0"));
        self.branch(&has_bitmap, &check, &sparse);
        self.start(&sparse);
        let sparse_count = self.instruction(format!(
            "call i64 @imre_struct_sparse_prefix(ptr {list}, ptr {rows}, ptr {cursor}, i64 {prefix})"
        ));
        let found = self.instruction(format!("icmp sge i64 {sparse_count}, 0"));
        self.branch(&found, &check, ordered);
        self.start(&check);
        let ones = self.instruction(format!(
            "phi i64 [{bitmap_count}, %{bitmap}], [{sparse_count}, %{sparse}]"
        ));
        let zeros = self.instruction(format!("sub i64 {prefix}, {ones}"));
        let initial = self.borrow_slot(plan.accumulator);
        let terms = [0, 1].map(|index| {
            let slot = self.instruction(format!("getelementptr ptr, ptr {rows}, i64 {index}"));
            let row = self.instruction(format!("load ptr, ptr {slot}"));
            self.dictionary_projection(plan.term, &row).operand
        });
        let mut sum = initial.operand.clone();
        let mut exact = String::from("true");
        for (term, frequency) in terms.iter().zip([zeros, ones]) {
            let fits = self.instruction(format!(
                "call i1 @imre_exact_fadd_bound(double {}, double {}, i64 {prefix})",
                initial.operand, term
            ));
            exact = self.instruction(format!("and i1 {exact}, {fits}"));
            let n = self.instruction(format!("uitofp i64 {frequency} to double"));
            let product = self.instruction(format!("fmul double {term}, {n}"));
            sum = self.instruction(format!("fadd double {sum}, {product}"));
        }
        let (fast, slow, advance) = (self.label(), self.label(), self.label());
        let inexact = self.label();
        self.branch(&exact, &fast, &inexact);
        self.start(&inexact);
        self.branch(&has_bitmap, &slow, ordered);
        self.start(&fast);
        self.jump(&advance);
        self.start(&slow);
        // Ordered additions can run for arbitrarily many rows. Publish the
        // loop state at the same checkpoint interval as a uniform scalar run.
        let cursor = self.borrow_slot(plan.cursor);
        let at = self.instruction(format!("lshr i64 {}, 1", cursor.operand));
        let remaining = self.instruction(format!("sub i64 {prefix}, {at}"));
        let short = self.instruction(format!("icmp ule i64 {remaining}, 65536"));
        let count = self.instruction(format!("select i1 {short}, i64 {remaining}, i64 65536"));
        let end = self.instruction(format!("add i64 {at}, {count}"));
        let subtotal = self.borrow_slot(plan.accumulator);
        let ordered_sum = self.instruction(format!(
            "call double @imre_struct_binary_ordered(ptr {rows}, i64 {at}, i64 {end}, double {}, double {}, double {})",
            subtotal.operand, terms[0], terms[1]
        ));
        self.jump(&advance);
        self.start(&advance);
        let end = self.instruction(format!("phi i64 [{prefix}, %{fast}], [{end}, %{slow}]"));
        let result = self.instruction(format!(
            "phi double [{sum}, %{fast}], [{ordered_sum}, %{slow}]"
        ));
        self.line(format!(
            "store double {result}, ptr %local{}",
            plan.accumulator
        ));
        let last = self.instruction(format!("sub i64 {end}, 1"));
        let row = self.instruction(format!("call ptr @imre_struct_row(ptr {list}, i64 {last})"));
        self.line(format!("store ptr {row}, ptr %local{}", plan.row));
        let shifted = self.instruction(format!("shl i64 {end}, 1"));
        let tagged = self.instruction(format!("or i64 {shifted}, 1"));
        self.line(format!("store i64 {tagged}, ptr %local{}", plan.cursor));
        let complete = self.instruction(format!("icmp eq i64 {end}, {prefix}"));
        let resume = self.label();
        self.branch(&complete, ordinary, &resume);
        self.start(&resume);
        self.check_interrupt();
        self.jump(&slow);
    }

    /// Execute the first cycle in source order. Each projected term must satisfy
    /// the same exact-addition bound for the entire remaining traversal. Failure
    /// resumes at the untouched suffix with the accumulator and row already correct.
    fn periodic_struct_sum(
        &mut self,
        plan: &RunSum<'_>,
        body: &[Statement],
        tagged_length: &str,
        mask: &str,
        ordinary: &str,
    ) {
        let cursor = self.borrow_slot(plan.cursor);
        let tagged = self.instruction(format!("trunc i64 {} to i1", cursor.operand));
        let at = self.instruction(format!("lshr i64 {}, 1", cursor.operand));
        let length = self.instruction(format!("lshr i64 {tagged_length}, 1"));
        let inside = self.instruction(format!("icmp ule i64 {at}, {length}"));
        let valid = self.instruction(format!("and i1 {tagged}, {inside}"));
        let remaining = self.instruction(format!("sub i64 {length}, {at}"));
        let period = self.instruction(format!("add i64 {mask}, 1"));
        let half = self.instruction(format!("lshr i64 {remaining}, 1"));
        let multiple = self.instruction(format!("icmp uge i64 {half}, {period}"));
        let enough = self.instruction(format!("and i1 {valid}, {multiple}"));
        let (start, visit, checked, skip) =
            (self.label(), self.label(), self.label(), self.label());
        self.branch(&enough, &start, ordinary);
        self.start(&start);
        let initial = self.borrow_slot(plan.accumulator);
        let end = self.instruction(format!("add i64 {at}, {period}"));
        let exact = self.entry_alloca("i1");
        self.line(format!("store i1 true, ptr {exact}"));
        self.jump(&visit);
        self.start(&visit);
        self.check_interrupt();
        self.block(&body[..2]);
        let sum = self.borrow_slot(plan.accumulator);
        let term = self.expr(plan.term);
        let added = self.instruction(format!("fadd double {}, {}", sum.operand, term.operand));
        self.line(format!(
            "store double {added}, ptr %local{}",
            plan.accumulator
        ));
        let fits = self.instruction(format!(
            "call i1 @imre_exact_fadd_bound(double {}, double {}, i64 {remaining})",
            initial.operand, term.operand
        ));
        let previous = self.instruction(format!("load i1, ptr {exact}"));
        let all = self.instruction(format!("and i1 {previous}, {fits}"));
        self.line(format!("store i1 {all}, ptr {exact}"));
        let next = self.borrow_slot(plan.cursor);
        let next = self.instruction(format!("lshr i64 {}, 1", next.operand));
        let complete = self.instruction(format!("icmp eq i64 {next}, {end}"));
        self.branch(&complete, &checked, &visit);
        self.start(&checked);
        self.branch(&all, &skip, ordinary);
        self.start(&skip);
        let rest = self.instruction(format!("sub i64 {length}, {end}"));
        let cycles = self.instruction(format!("udiv i64 {rest}, {period}"));
        let count = self.instruction(format!("uitofp i64 {cycles} to double"));
        let delta = self.instruction(format!("fsub double {added}, {}", initial.operand));
        let product = self.instruction(format!("fmul double {delta}, {count}"));
        let bulk = self.instruction(format!("fadd double {added}, {product}"));
        // A zero-delta cycle has reached a fixed point, including the sign of zero.
        let fixed = self.instruction(format!("fcmp oeq double {delta}, 0.0"));
        let result = self.instruction(format!("select i1 {fixed}, double {added}, double {bulk}"));
        self.line(format!(
            "store double {result}, ptr %local{}",
            plan.accumulator
        ));
        let tail = self.instruction(format!("and i64 {rest}, {mask}"));
        let resume = self.instruction(format!("sub i64 {length}, {tail}"));
        let shifted = self.instruction(format!("shl i64 {resume}, 1"));
        let tagged = self.instruction(format!("or i64 {shifted}, 1"));
        self.line(format!("store i64 {tagged}, ptr %local{}", plan.cursor));
        self.jump(ordinary);
    }
}

/// Guard signed tagged range endpoints and a positive modulus before any list access.
const INDEX_RANGE_HELPER: &str = r#"
define internal i1 @imre_uniform_index_range(i64 %start, i64 %stop, i64 %modulus, i64 %limit) alwaysinline nounwind {
  %ends = and i64 %start, %stop
  %tags = and i64 %ends, %modulus
  %small = trunc i64 %tags to i1
  %nonnegative = icmp sge i64 %start, 1
  %positive = icmp sgt i64 %modulus, 1
  %nonempty = icmp slt i64 %start, %stop
  %end = lshr i64 %stop, 1
  %last = sub i64 %end, 1
  %bounded = icmp ule i64 %last, %limit
  %domain = and i1 %positive, %nonnegative
  %range = and i1 %nonempty, %bounded
  %valid = and i1 %domain, %range
  %result = and i1 %small, %valid
  ret i1 %result
}

; The limit and distance are logical iteration counts. Cursor increments use tagged units.
define internal i64 @imre_modular_batch_width(i64 %cursor, i64 %left, i64 %count, ptr %runs) alwaysinline nounwind {
  %sparse = icmp sgt i64 %count, 0
  br i1 %sparse, label %exceptions, label %fixed
fixed:
  %uniform = icmp eq i64 %count, 0
  %constant = select i1 %uniform, i64 2046, i64 2
  br label %limit
exceptions:
  %ordinal = lshr i64 %cursor, 1
  %distance = call i64 @imre_struct_modular_span(ptr %runs, i64 %count, i64 %ordinal, i64 1023)
  %width = shl i64 %distance, 1
  br label %limit
limit:
  %cap = phi i64 [ %constant, %fixed ], [ %width, %exceptions ]
  %short = icmp ult i64 %left, %cap
  %span = select i1 %short, i64 %left, i64 %cap
  ret i64 %span
}

; The range proof covers both the multiply and the addition. Positive Python
; modulo agrees with unsigned remainder throughout this nonnegative domain.
define internal i64 @imre_bounded_modular_index(i64 %tag, i64 %scale, i64 %offset, i64 %modulus) alwaysinline nounwind {
  %i = lshr i64 %tag, 1
  %n = lshr i64 %modulus, 1
  %product = mul nuw nsw i64 %i, %scale
  %value = add nuw nsw i64 %product, %offset
  %index = urem i64 %value, %n
  %shifted = shl i64 %index, 1
  %tagged = or i64 %shifted, 1
  ret i64 %tagged
}
"#;

const STRUCT_RUN_HELPERS: &str = r"
; The cursor holds a valid, nonempty span after a sparse row read. A dense
; span or a boxed single row retains the original one-element advance.
define internal i64 @imre_struct_uniform_end(ptr %span, i64 %next) alwaysinline nounwind {
  %lp = getelementptr inbounds {i64,i64,i64,ptr}, ptr %span, i32 0, i32 1
  %mp = getelementptr inbounds {i64,i64,i64,ptr}, ptr %span, i32 0, i32 2
  %start = load i64, ptr %span
  %len = load i64, ptr %lp
  %mask = load i64, ptr %mp
  %end = add i64 %start, %len
  %repeat = icmp eq i64 %mask, 0
  %selected = select i1 %repeat, i64 %end, i64 %next
  ret i64 %selected
}

; Count the two values in a sparse prefix using the existing indexed reduction.
; A long initial span is cheaper to consume directly. Leave it in the read
; cursor for the ordinary traversal. Short packed spans indicate scattered edits.
define internal i64 @imre_struct_sparse_prefix(ptr %list, ptr %rows, ptr %cursor, i64 %prefix) noinline nounwind {
entry:
  %short = icmp ult i64 %prefix, 64
  %large = icmp ugt i64 %prefix, 4294967296
  %outside = or i1 %short, %large
  br i1 %outside, label %miss, label %first
first:
  call void @imre_struct_seek_span(ptr %list, i64 0, ptr %cursor)
  %length = getelementptr inbounds {i64,i64,i64,ptr}, ptr %cursor, i32 0, i32 1
  %end = load i64, ptr %length
  %threshold = lshr i64 %prefix, 5
  %long = icmp ugt i64 %end, %threshold
  br i1 %long, label %miss, label %inspect
inspect:
  %mask = getelementptr inbounds {i64,i64,i64,ptr}, ptr %cursor, i32 0, i32 2
  %initial = load i64, ptr %mask
  %packed = icmp ne i64 %initial, 0
  br i1 %packed, label %count, label %second
second:
  call void @imre_struct_seek_span(ptr %list, i64 %end, ptr %cursor)
  %following = load i64, ptr %mask
  %scattered = icmp ne i64 %following, 0
  br i1 %scattered, label %count, label %miss
count:
  %ones = call i64 @imre_struct_sparse_indexed_count(ptr %list, ptr %rows, i64 %prefix, i64 0, i64 1, i64 %prefix)
  ret i64 %ones
miss:
  ret i64 -1
}

; Bound the absolute value of every partial sum for an integral projection.
; The strict comparison also rejects a rounded-up or rounded-down boundary.
define internal i1 @imre_exact_fadd_bound(double %sum, double %term, i64 %count) alwaysinline nounwind {
  %n = uitofp i64 %count to double
  %ni = icmp ule i64 %count, 9007199254740992
  %t = call double @llvm.trunc.f64(double %term)
  %s = call double @llvm.trunc.f64(double %sum)
  %ti = fcmp oeq double %term, %t
  %si = fcmp oeq double %sum, %s
  %a = call double @llvm.fabs.f64(double %sum)
  %b = call double @llvm.fabs.f64(double %term)
  %delta = fmul double %b, %n
  %limit = fadd double %a, %delta
  %bounded = fcmp olt double %limit, 9007199254740992.0
  %integral = and i1 %ti, %si
  %fits = and i1 %ni, %bounded
  %exact = and i1 %integral, %fits
  ret i1 %exact
}

; Reassociation is exact only if the integral term, count and every partial
; sum fit in binary64's consecutive integer range. Otherwise preserve each add.
define internal double @imre_repeat_fadd(double %sum, double %term, i64 %count) alwaysinline nounwind {
entry:
  %single = icmp eq i64 %count, 1
  br i1 %single, label %one, label %bounds
one:
  %single_sum = fadd double %sum, %term
  ret double %single_sum
bounds:
  %exact = call i1 @imre_exact_fadd_bound(double %sum, double %term, i64 %count)
  br i1 %exact, label %bulk, label %ordered
bulk:
  %n = uitofp i64 %count to double
  %product = fmul double %term, %n
  %result = fadd double %sum, %product
  ret double %result
ordered:
  %left = phi i64 [%count, %bounds], [%remaining, %ordered]
  %acc = phi double [%sum, %bounds], [%added, %ordered]
  %added = fadd double %acc, %term
  %remaining = sub i64 %left, 1
  %done = icmp eq i64 %remaining, 0
  br i1 %done, label %finish, label %ordered
finish:
  ret double %added
}
";

#[cfg(test)]
mod tests {
    #[test]
    fn modular_index_batches_require_independent_pure_row_projections() {
        let prelude = "from imre import struct\n@struct\nclass Row:\n    x: float\n";
        let compile = |body: &str| {
            let source = format!(
                "{prelude}def f(rows: list[Row], n: int) -> float:\n    total = 0.0\n    for i in range(n):\n{body}    return total\nprint(f([Row(1.5)] * 4, 4))"
            );
            crate::compile(&source, &crate::CompileOptions::default()).unwrap()
        };
        let eligible = compile("        at = (7 * i + 1) % n\n        total += rows[at].x\n");
        assert!(eligible.contains("call i1 @imre_uniform_index_range("));
        for body in [
            "        at = i % n\n        total += total * rows[at].x\n",
            "        at = i % n\n        total += 1.0 / rows[at].x\n",
            "        at = i % n\n        total += rows[i].x\n",
            "        n = (i + 1) % n\n        total += rows[n].x\n",
            "        at = i % n\n        rows[at] = Row(4.0)\n        total += rows[at].x\n",
        ] {
            assert!(
                !compile(body).contains("call i1 @imre_uniform_index_range("),
                "{body}"
            );
        }
    }

    #[test]
    fn run_folds_require_a_pure_independent_float_term() {
        let source = r#"
from imre import struct
@struct
class Reading:
    x: float
    def scaled(self) -> float:
        return self.x * 0.5
    def noisy(self) -> float:
        print(self.x)
        return self.x
def eligible(rows: list[Reading], offset: float) -> float:
    total = 0.0
    for item in rows:
        total += item.scaled() - offset
    return total
def dependent(rows: list[Reading]) -> float:
    total = 1.0
    for item in rows:
        total += total * item.x
    return total
def effectful(rows: list[Reading]) -> float:
    total = 0.0
    for item in rows:
        total += item.noisy()
    return total
def fallible(rows: list[Reading]) -> float:
    total = 0.0
    for item in rows:
        total += 1.0 / item.x
    return total
rows = [Reading(2.0)] * 3
print(eligible(rows, 1.0))
print(dependent(rows))
print(effectful(rows))
print(fallible(rows))
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call double @imre_repeat_fadd(").count(), 1);
        // Only the independent projection can query a two-value dictionary.
        assert_eq!(
            llvm.matches("call i64 @imre_struct_sparse_prefix(").count(),
            1
        );
        assert_eq!(llvm.matches("call i1 @imre_exact_fadd_bound(").count(), 5);
    }
}
