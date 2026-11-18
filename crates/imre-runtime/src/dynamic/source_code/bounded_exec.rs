//! A deliberately small, bounded runtime statement plan for guarded file-source execution.

use super::*;

const MAX_SOURCE_BYTES: usize = 1 << 20;
const MAX_STATEMENTS: usize = 512;
const MAX_SUITE_DEPTH: usize = 128;

#[derive(Clone, Copy)]
enum Literal {
    None,
    Bool(bool),
    SmallInt(i64),
}

enum Operation {
    If {
        condition: bool,
        end: usize,
        line: u32,
    },
    Store {
        name: String,
        value: Literal,
        line: u32,
    },
    Pass {
        line: u32,
    },
}

/// An owned operation list contains no Python heap handles and is bounded before execution.
pub(in crate::dynamic) struct Plan {
    operations: Vec<Operation>,
}

enum Work<'a> {
    Statement(&'a ast::Stmt, usize),
    EndIf(usize),
}

fn allocation() -> Error {
    Error::new("MemoryError", "")
}

/// Schedule a suite iteratively without letting pending siblings exceed the plan budget.
fn push_suite<'a>(work: &mut Vec<Work<'a>>, suite: &'a [ast::Stmt], depth: usize) -> Result<bool> {
    if depth > MAX_SUITE_DEPTH
        || suite.len() > MAX_STATEMENTS
        || work.len().saturating_add(suite.len()) > MAX_STATEMENTS + MAX_SUITE_DEPTH
    {
        return Ok(false);
    }
    work.try_reserve(suite.len()).map_err(|_| allocation())?;
    work.extend(
        suite
            .iter()
            .rev()
            .map(|statement| Work::Statement(statement, depth)),
    );
    Ok(true)
}

impl Literal {
    fn from_expression(expression: &ast::Expr) -> Option<Self> {
        match expression {
            ast::Expr::NoneLiteral(_) => Some(Self::None),
            ast::Expr::BooleanLiteral(value) => Some(Self::Bool(value.value)),
            ast::Expr::NumberLiteral(value) => {
                let ast::Number::Int(value) = &value.value else {
                    return None;
                };
                let value = value.as_i64()?;
                // Restrict execution to the permanent nonnegative small-integer handles.
                (0..=256).contains(&value).then_some(Self::SmallInt(value))
            }
            _ => None,
        }
    }

    fn realize(self, context: &mut Context) -> Id {
        match self {
            Self::None => NONE,
            Self::Bool(true) => TRUE,
            Self::Bool(false) => FALSE,
            Self::SmallInt(value) => context.int(value),
        }
    }
}

impl Plan {
    /// Validate the complete source before publishing any executable operation.
    fn prepare(source: &str) -> Result<Option<Self>> {
        if source.len() > MAX_SOURCE_BYTES {
            return Ok(None);
        }
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let normalized = normalized_source(source);
            let input = imre_ast::indentation::prepare(&normalized);
            let parsed = ruff_python_parser::parse_unchecked(
                &input.text,
                ruff_python_parser::Mode::Module.into(),
            );
            if !parsed.errors().is_empty() {
                return Ok(None);
            }
            let ast::Mod::Module(module) = parsed.syntax() else {
                return Ok(None);
            };
            let lines = line_starts(&input.text)?;
            let mut work = Vec::new();
            let mut operations = Vec::new();
            if !push_suite(&mut work, &module.body, 0)? {
                return Ok(None);
            }
            while let Some(item) = work.pop() {
                match item {
                    Work::EndIf(index) => {
                        let end = operations.len();
                        let Operation::If { end: slot, .. } = &mut operations[index] else {
                            unreachable!()
                        };
                        *slot = end;
                    }
                    Work::Statement(statement, depth) => {
                        if operations.len() >= MAX_STATEMENTS {
                            return Ok(None);
                        }
                        operations.try_reserve(1).map_err(|_| allocation())?;
                        let line = lines
                            .partition_point(|&offset| offset <= statement.start().to_usize())
                            as u32;
                        match statement {
                            ast::Stmt::If(value) => {
                                let ast::Expr::BooleanLiteral(condition) = value.test.as_ref()
                                else {
                                    return Ok(None);
                                };
                                if !value.elif_else_clauses.is_empty() {
                                    return Ok(None);
                                }
                                let index = operations.len();
                                operations.push(Operation::If {
                                    condition: condition.value,
                                    end: 0,
                                    line,
                                });
                                work.try_reserve(1).map_err(|_| allocation())?;
                                work.push(Work::EndIf(index));
                                if !push_suite(&mut work, &value.body, depth + 1)? {
                                    return Ok(None);
                                }
                            }
                            ast::Stmt::Assign(value) => {
                                let [ast::Expr::Name(name)] = value.targets.as_slice() else {
                                    return Ok(None);
                                };
                                let Some(value) = Literal::from_expression(&value.value) else {
                                    return Ok(None);
                                };
                                let mut binding = String::new();
                                binding
                                    .try_reserve_exact(name.id.as_str().len())
                                    .map_err(|_| allocation())?;
                                binding.push_str(name.id.as_str());
                                operations.push(Operation::Store {
                                    name: binding,
                                    value,
                                    line,
                                });
                            }
                            ast::Stmt::Pass(_) => operations.push(Operation::Pass { line }),
                            _ => return Ok(None),
                        }
                    }
                }
            }
            Ok(Some(Self { operations }))
        })
    }

    /// Run checked operations in the normal exec frame, publishing every active source line.
    fn execute(&self, context: &mut Context, scope: Id) -> Result<Id> {
        let mut index = 0;
        while let Some(operation) = self.operations.get(index) {
            let line = match operation {
                Operation::If { line, .. }
                | Operation::Store { line, .. }
                | Operation::Pass { line } => *line,
            };
            // SAFETY: the entered code frame owns this source, so a null filename selects it.
            unsafe { context.native_statement(&[], std::ptr::null(), line, NONE) }?;
            match operation {
                Operation::If {
                    condition: false,
                    end,
                    ..
                } => {
                    index = *end;
                    continue;
                }
                Operation::Store { name, value, .. } => {
                    let value = value.realize(context);
                    context.bind_source_name(scope, name, value)?;
                }
                _ => {}
            }
            index += 1;
        }
        Ok(NONE)
    }
}

/// Execute only a fully validated plan installed by the guarded operand path.
///
/// # Safety
/// The interpreter entered the code object associated with its live `BoundedExec` plan.
unsafe extern "C" fn entry(context: *mut Context, scope: Id) -> Id {
    // SAFETY: native code entries receive the active interpreter for their entered frame.
    let context = unsafe { &mut *context };
    let code = context.heap.current_code();
    let plan = match context.heap.linked_sources.get(&code) {
        Some(LinkedSource::BoundedExec(plan)) => Rc::clone(plan),
        _ => {
            return ffi::finish_at(
                context,
                Err(Error::new(
                    "NotImplementedError",
                    "bounded execution plan is unavailable",
                )),
                None,
            );
        }
    };
    let result = plan.execute(context, scope);
    ffi::finish_at(context, result, None)
}

impl Context {
    /// Install an executable entry only after the entire immutable source passes the bounded plan.
    pub(in crate::dynamic) fn link_bounded_file_exec(
        &mut self,
        callable: Id,
        code: Id,
        flags: u32,
    ) -> Result<Id> {
        if !matches!(self.heap.get(callable), Value::Native(name) if name == "exec") {
            return Ok(code);
        }
        let Value::Code(object) = self.heap.get(code) else {
            return Ok(code);
        };
        let Body::Parsed { constants, source } = &object.body else {
            return Ok(code);
        };
        let (constants, source) = (*constants, *source);
        if object.definition.flags != flags || !matches!(self.heap.get(source), Value::Str(_)) {
            return Ok(code);
        }
        let Some(plan) = Plan::prepare(self.str_value(source)?)? else {
            return Ok(code);
        };
        self.heap
            .linked_sources
            .try_reserve(1)
            .map_err(|_| allocation())?;
        self.heap
            .linked_sources
            .insert(code, LinkedSource::BoundedExec(Rc::new(plan)));
        let Value::Code(object) = self.heap.get_mut(code) else {
            unreachable!("checked code")
        };
        object.body = Body::Native(entry, constants);
        Ok(code)
    }
}
