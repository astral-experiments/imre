//! A cached getter owns its successful result on the receiving instance.
//! Hidden fields follow the public layout and are addressed only after ordinary
//! property dispatch has selected the concrete class.

use super::*;

pub(super) type Slots = HashMap<(usize, usize), (usize, usize)>;

pub(super) fn slots(
    graph: &SourceGraph,
    types: &Types,
    classes: &Classes,
    fields: &mut [Vec<Type>],
) -> Result<Slots, String> {
    let mut slots = Slots::new();
    for (class, declaration) in classes.definitions.iter().enumerate() {
        for (method, (node, origin)) in declaration
            .methods
            .iter()
            .zip(&declaration.method_origins)
            .enumerate()
        {
            if !matches!(node.decorator_list.as_slice(), [d] if graph.resolve(origin.module, &d.expression) == Some(Binding::Intrinsic(Intrinsic::CachedProperty)))
            {
                continue;
            }
            let module = &graph.modules[origin.module];
            if declaration.dataclass.is_some() || declaration.enum_members.is_some() {
                return Err(module.error(node, "cached_property requires an ordinary fixed class"));
            }
            let result = node
                .returns
                .as_deref()
                .ok_or_else(|| module.error(node, "a cached_property needs an annotated result"))?;
            let ty =
                annotation_in_class(result, graph, origin.module, classes, types, Some(class))?;
            if callables::contains_callable(ty, types) {
                return Err(
                    module.error(result, "cached properties cannot contain callable values")
                );
            }
            let ready = fields[class].len();
            fields[class].extend([Type::Bool, ty]);
            slots.insert((class, method), (ready, ready + 1));
        }
    }
    Ok(slots)
}

impl Checker<'_> {
    pub(super) fn cached_property(
        &mut self,
        receiver: Expr,
        getter: &Signature,
        ready: usize,
        value: usize,
    ) -> Expr {
        let owner = self.hidden(receiver.ty);
        let result = self.hidden(getter.returns);
        let field = |slot, ty| Expr {
            ty,
            kind: ExprKind::Field(Box::new(self.local(owner)), slot),
        };
        let compute = Expr {
            ty: getter.returns,
            kind: ExprKind::Call(getter.id, ir::CallArgs::positional(vec![self.local(owner)])),
        };
        Expr {
            ty: getter.returns,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(owner, receiver),
                    Statement::If(
                        field(ready, Type::Bool),
                        vec![Statement::Assign(result, field(value, getter.returns))],
                        vec![
                            Statement::Assign(result, compute),
                            Statement::SetField(self.local(owner), value, self.local(result)),
                            Statement::SetField(
                                self.local(owner),
                                ready,
                                Expr {
                                    ty: Type::Bool,
                                    kind: ExprKind::Bool(true),
                                },
                            ),
                        ],
                    ),
                ],
                value: Box::new(self.local(result)),
                clear: vec![owner, result],
            },
        }
    }
}
