//! The filesystem cache exposes a real typed dict and its ordinary mutable str
//! lists. Loader objects need callable dispatch and are not compiled here.
use super::*;
use ir::LinecacheOperation as Op;

impl Checker<'_> {
    pub(super) fn linecache_entry(&self) -> Type {
        self.types.tuple(vec![
            Type::Int,
            Type::Float,
            self.types.list(Type::Str),
            Type::Str,
        ])
    }

    pub(super) fn linecache_mapping(&self) -> Expr {
        let entry = self.linecache_entry();
        Expr {
            ty: self.types.dict(Type::Str, entry),
            kind: ExprKind::Builtin(Builtin::Linecache(Op::Cache, entry, false), vec![]),
        }
    }

    pub(super) fn linecache_call(&mut self, op: Op, call: &ast::ExprCall) -> Result<Expr, String> {
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let names: &[(&str, bool)] = match op {
            Op::Getline => &[
                ("filename", false),
                ("lineno", false),
                ("module_globals", true),
            ],
            Op::Getlines | Op::Update => &[("filename", false), ("module_globals", true)],
            Op::Lazy => &[("filename", false), ("module_globals", false)],
            Op::Check => &[("filename", true)],
            Op::Clear => &[],
            Op::Cache => unreachable!(),
        };
        let params = names
            .iter()
            .map(|(name, optional)| Parameter {
                name: (*name).into(),
                ty: Type::None,
                kind: ParameterKind::PositionalOrKeyword,
                default: optional.then(|| none.clone()),
                sign_only: false,
            })
            .collect::<Vec<_>>();
        let mut args=self.arguments_with(call,&params,None,|c,i,node|{
            let v=Self::string_value(c.expr(node)?);
            match names[i].0 {
                "filename" if v.ty == Type::Str || (op == Op::Check && v.ty == Type::None) => Ok(v),
                "filename" => Err(error(c.source,node,"linecache filename requires a string")),
                "lineno" if matches!(v.ty,Type::Int|Type::Float)=>Ok(v),
                "lineno" if v.ty == Type::Bool=> Ok(Expr{ty:Type::Int,kind:ExprKind::Builtin(Builtin::Int,vec![v])}),
                "lineno" => Err(error(c.source,node,"linecache line number requires int, bool or float")),
                _ if v.ty==Type::None || c.types.mapping(v.ty)==Some((Type::Str,Type::Str)) => Ok(v),
                _ => Err(error(c.source,node,"linecache source loaders need a supported callable; only None or dict[str, str] globals are supported")),
            }
        })?;
        let returns = match op {
            Op::Getlines | Op::Update => self.types.list(Type::Str),
            Op::Getline => Type::Str,
            Op::Lazy => Type::Bool,
            _ => Type::None,
        };
        let (root, environment) = self.linecache_search();
        if matches!(op, Op::Getline | Op::Getlines | Op::Update) {
            // A script's own directory precedes the immutable startup PYTHONPATH.
            // -c uses the current directory (already tried by the direct stat).
            args.order.push(args.values.len());
            args.values.push(root);
            // CPython's '\n' and '' belong to the same literal pool as a
            // caller's strings; using new equal strings would break identity.
            for value in std::iter::once("\n").chain((op == Op::Getline).then_some("")) {
                args.order.push(args.values.len());
                args.values.push(unpacking::string(value));
            }
        }
        let entry = self.linecache_entry();
        Ok(self.ordered_builtin(Builtin::Linecache(op, entry, environment), returns, args))
    }

    /// Direct linecache calls and implicit warning source lookup search the same paths.
    pub(super) fn linecache_search(&self) -> (Expr, bool) {
        let command = self.graph.command_startup.as_ref().map(|(flags, _)| flags);
        let path = if command.is_none() {
            self.graph.modules[self.graph.entry]
                .path
                .parent()
                .and_then(std::path::Path::to_str)
                .unwrap_or("")
        } else {
            ""
        };
        (
            unpacking::string(path),
            !command.is_some_and(|flags| flags.isolated),
        )
    }
}
