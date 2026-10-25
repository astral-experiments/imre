//! Plain class data uses one global. Only written names need a rebindable cell and per-instance storage.
use super::*;
#[derive(Clone, Copy)]
struct Cell {
    slot: usize,
    ty: Type,
    inherited: bool,
}
#[derive(Clone, Copy)]
struct Shadow {
    ready: usize,
    value: usize,
    ty: Type,
    stored: Type,
}
#[derive(Default)]
pub(super) struct Storage {
    cells: HashMap<(usize, String), Cell>,
    shadows: HashMap<(usize, String), Shadow>,
    fields: HashMap<usize, Vec<Type>>,
}
impl Storage {
    pub(super) fn extend_layouts(&mut self, layouts: &mut [Vec<Type>]) {
        for (c, f) in self.fields.drain() {
            layouts[c].extend(f);
        }
    }
}
fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}
fn boolean(v: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(v),
    }
}
fn scope(body: Vec<Statement>, value: Expr, clear: Vec<usize>) -> Expr {
    Expr {
        ty: value.ty,
        kind: ExprKind::Scope {
            body,
            value: Box::new(value),
            clear,
        },
    }
}
fn field(owner: &Expr, at: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Field(Box::new(owner.clone()), at),
    }
}
fn choose(test: Expr, yes: Expr, no: Expr) -> Expr {
    Expr {
        ty: yes.ty,
        kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
    }
}
impl Checker<'_> {
    pub(super) fn class_storage_value(
        &self,
        name: &str,
        value: Expr,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let Some(writes) = self.classes.declarations.attribute_writes.get(name) else {
            return Ok(value);
        };
        let mut ty = value.ty;
        if ty == Type::None && writes.len() == 1 {
            ty = self.types.optional(*writes.iter().next().unwrap());
        }
        let value = self.coerce(value, ty, at)?;
        Ok(Expr {
            ty: self.types.list(ty),
            kind: ExprKind::List(vec![value]),
        })
    }
    pub(super) fn record_class_cell(&self, class: usize, name: &str, slot: usize, storage: Type) {
        if !self
            .classes
            .declarations
            .attribute_writes
            .contains_key(name)
        {
            return;
        }
        let ty = self.types.element(storage).unwrap();
        self.globals.class_data.borrow_mut().cells.insert(
            (class, name.to_owned()),
            Cell {
                slot,
                ty,
                inherited: false,
            },
        );
    }
    fn cell(&self, class: usize, name: &str) -> Option<Cell> {
        self.globals
            .class_data
            .borrow()
            .cells
            .get(&(class, name.to_owned()))
            .copied()
    }
    pub(super) fn class_data_type(&self, class: usize, name: &str) -> Option<Type> {
        self.cell(class, name).map(|cell| cell.ty)
    }
    /// A subclass can gain a new local class binding after its body returns.
    pub(super) fn inherit_class_data(
        &self,
        class: usize,
        body: &mut Vec<Statement>,
    ) -> Result<(), String> {
        let def = &self.classes.declarations.definitions[class];
        let mut names: Vec<_> = def
            .static_bindings
            .iter()
            .filter_map(|(n, b)| {
                (matches!(b, crate::classes::ClassValue::Runtime { .. })
                    && self.classes.declarations.attribute_writes.contains_key(n))
                .then_some(n)
            })
            .collect();
        names.sort_unstable();
        let mut data = self.globals.class_data.borrow_mut();
        for name in names {
            let key = (class, name.clone());
            let cell = if let Some(c) = data.cells.get(&key) {
                *c
            } else {
                let ty = def
                    .bases
                    .iter()
                    .find_map(|b| data.cells.get(&(*b, name.clone())))
                    .map(|c| c.ty)
                    .ok_or_else(|| {
                        error(
                            self.source,
                            &def.node,
                            "inherited class data is not initialized",
                        )
                    })?;
                let st = self.types.list(ty);
                let c = Cell {
                    slot: self.globals.reserve(st),
                    ty,
                    inherited: true,
                };
                body.push(Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::GlobalSet(c.slot),
                        vec![Expr {
                            ty: st,
                            kind: ExprKind::List(vec![]),
                        }],
                    ),
                }));
                data.cells.insert(key.clone(), c);
                c
            };
            let extra = data.fields.entry(class).or_default();
            let ready = self.classes.fields[class].len() + extra.len();
            let stored = self.types.optional(cell.ty);
            extra.extend([Type::Bool, stored]);
            data.shadows.insert(
                key,
                Shadow {
                    ready,
                    value: ready + 1,
                    ty: cell.ty,
                    stored,
                },
            );
        }
        Ok(())
    }
    fn shadow(&self, class: usize, name: &str) -> Option<Shadow> {
        self.globals
            .class_data
            .borrow()
            .shadows
            .get(&(class, name.to_owned()))
            .copied()
    }
    fn cell_item(&self, c: Cell) -> Expr {
        Expr {
            ty: c.ty,
            kind: ExprKind::Builtin(
                Builtin::ListGet,
                vec![self.globals.slot_value(c.slot), pipelines::integer(0)],
            ),
        }
    }
    fn nonempty(&self, c: Cell) -> Expr {
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::Bool, vec![self.globals.slot_value(c.slot)]),
        }
    }
    /// Read the nearest class binding unless this exact instance has its own value.
    pub(super) fn data_attribute(
        &mut self,
        class: usize,
        attr: &ast::ExprAttribute,
        receiver: Option<Expr>,
        original: Expr,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        let Some(actual) = self.cell(class, name) else {
            if self
                .classes
                .declarations
                .attribute_writes
                .contains_key(name)
            {
                return Err(error(
                    self.source,
                    attr,
                    "class value is read before its declaration",
                ));
            }
            return Ok(self.evaluated_receiver(receiver, original));
        };
        let mut value = None;
        for &owner in self.classes.declarations.definitions[class]
            .bases
            .iter()
            .rev()
            .chain(std::iter::once(&class))
        {
            let Some(cell) = self.cell(owner, name) else {
                continue;
            };
            let item = self.cell_item(cell);
            value = Some(if cell.inherited {
                let base = value.ok_or_else(|| {
                    error(self.source, attr, "inherited class data is not initialized")
                })?;
                choose(self.nonempty(cell), item, base)
            } else {
                item
            });
        }
        let value = value.unwrap();
        let Some(receiver) = receiver else {
            return Ok(value);
        };
        let s = self
            .shadow(class, name)
            .ok_or_else(|| error(self.source, attr, "class data layout is not initialized"))?;
        let slot = self.hidden(receiver.ty);
        let local = self.local(slot);
        let payload = field(&local, s.value, s.stored);
        let payload = if s.stored == actual.ty {
            payload
        } else {
            Expr {
                ty: actual.ty,
                kind: ExprKind::Unwrap(Box::new(payload)),
            }
        };
        Ok(scope(
            vec![Statement::Assign(slot, receiver)],
            choose(field(&local, s.ready, Type::Bool), payload, value),
            vec![slot],
        ))
    }
    pub(super) fn store_class_data(
        &mut self,
        class: usize,
        attr: &ast::ExprAttribute,
        value: Expr,
    ) -> Result<Statement, String> {
        let c = self
            .cell(class, attr.attr.as_str())
            .ok_or_else(|| error(self.source, attr, "class data is not initialized"))?;
        self.class_reads
            .borrow_mut()
            .insert(self.classes.declarations.definitions[class].definition);
        let value = self.coerce(value, c.ty, attr)?;
        let slot = self.hidden(value.ty);
        let cell = self.globals.slot_value(c.slot);
        let set = Statement::SetItem(cell.clone(), pipelines::integer(0), self.local(slot));
        let store = if c.inherited {
            let append = Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::Append, vec![cell, self.local(slot)]),
            });
            Statement::If(self.nonempty(c), vec![set], vec![append])
        } else {
            set
        };
        Ok(Statement::Expr(scope(
            vec![Statement::Assign(slot, value), store],
            none(),
            vec![slot],
        )))
    }
    pub(super) fn mutable_instance_data(&self, ty: Type, name: &str) -> Option<usize> {
        let Type::Class(class) = ty else { return None };
        self.shadow(class, name).map(|_| class)
    }
    fn writable_shadow(&self, class: usize, attr: &ast::ExprAttribute) -> Result<Shadow, String> {
        if self.classes.declarations.definitions[class]
            .dataclass
            .is_some_and(|options| options.frozen)
        {
            return Err(error(
                self.source,
                attr,
                "frozen dataclass attributes cannot be reassigned",
            ));
        }
        self.shadow(class, attr.attr.as_str())
            .ok_or_else(|| error(self.source, attr, "class data layout is not initialized"))
    }
    /// Assignment evaluates its right hand side before its target, including on failure.
    pub(super) fn store_instance_data(
        &mut self,
        class: usize,
        attr: &ast::ExprAttribute,
        receiver: Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        let slot = self.hidden(value.ty);
        let branch = self.dispatch_record(class, receiver, attr, |me, concrete, view| {
            let s = me.writable_shadow(concrete, attr)?;
            let v = me.coerce(me.local(slot), s.ty, attr)?;
            let v = me.coerce(v, s.stored, attr)?;
            Ok(scope(
                vec![
                    Statement::SetField(view.clone(), s.value, v),
                    Statement::SetField(view, s.ready, boolean(true)),
                ],
                none(),
                Vec::new(),
            ))
        })?;
        Ok(Statement::Expr(scope(
            vec![Statement::Assign(slot, value)],
            branch,
            vec![slot],
        )))
    }
    pub(super) fn delete_instance_data(
        &mut self,
        class: usize,
        attr: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<Statement, String> {
        let name = attr.attr.as_str();
        self.dispatch_record(class, receiver, attr, |me, concrete, view| {
            let s = me.writable_shadow(concrete, attr)?;
            let release = me.coerce(none(), s.stored, attr)?;
            let cls = &me.classes.declarations.definitions[concrete].name;
            let exc = Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("AttributeError").unwrap(),
                    message: Some(Box::new(unpacking::string(format!(
                        "'{cls}' object has no attribute '{name}'"
                    )))),
                },
            };
            Ok(scope(
                vec![Statement::If(
                    field(&view, s.ready, Type::Bool),
                    vec![
                        Statement::SetField(view.clone(), s.value, release),
                        Statement::SetField(view, s.ready, boolean(false)),
                    ],
                    vec![Statement::Raise(Some(exc))],
                )],
                none(),
                Vec::new(),
            ))
        })
        .map(Statement::Expr)
    }
}
