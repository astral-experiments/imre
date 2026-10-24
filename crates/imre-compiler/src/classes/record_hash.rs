//! Recognize a pure hash projection of a frozen record. Its fields are checked as
//! immutable JSON object views after nominal annotation resolution.

use super::*;

pub(super) fn toml_fields(
    method: &ast::StmtFunctionDef,
    fields: &[Field],
    graph: &SourceGraph,
    module: usize,
) -> bool {
    let Some(receiver) = method.parameters.args.first() else {
        return false;
    };
    if method.is_async
        || !method.decorator_list.is_empty()
        || method.parameters.len() != 1
        || receiver.default.is_some()
        || receiver.name().as_str() != "self"
        || !matches!(method.returns.as_deref(), Some(ast::Expr::Name(name)) if name.id == "int")
        || graph.modules[module].bindings.contains_key("int")
        || graph.modules[module].bindings.contains_key("hash")
    {
        return false;
    }
    let body = method.body.as_slice();
    let body = if matches!(body.first(), Some(ast::Stmt::Expr(stmt)) if matches!(*stmt.value, ast::Expr::StringLiteral(_)))
    {
        &body[1..]
    } else {
        body
    };
    let Some(ast::Stmt::Return(last)) = body.last() else {
        return false;
    };
    let Some(ast::Expr::Call(hash)) = last.value.as_deref() else {
        return false;
    };
    let [operand] = hash.arguments.args.as_slice() else {
        return false;
    };
    if !matches!(&*hash.func, ast::Expr::Name(name) if name.id == "hash")
        || !hash.arguments.keywords.is_empty()
    {
        return false;
    }
    let value = match body {
        [_] => operand,
        [ast::Stmt::FunctionDef(helper), _]
            if !helper.is_async
                && helper.parameters.is_empty()
                && helper.name.as_str() != "hash"
                && helper.name.as_str() != receiver.name().as_str()
                && helper.type_params.is_none()
                && helper.returns.is_none()
                && helper.decorator_list.len() == 1
                && graph.resolve(module, &helper.decorator_list[0].expression)
                    == Some(Binding::Intrinsic(Intrinsic::FunctionCache))
                && matches!(operand, ast::Expr::Call(call) if call.arguments.is_empty() && matches!(&*call.func, ast::Expr::Name(name) if name.id == helper.name.id)) =>
        {
            let [ast::Stmt::Return(ret)] = helper.body.as_slice() else {
                return false;
            };
            let Some(value) = ret.value.as_deref() else {
                return false;
            };
            value
        }
        _ => return false,
    };
    let ast::Expr::Call(dump) = value else {
        return false;
    };
    let [ast::Expr::Dict(dictionary)] = dump.arguments.args.as_slice() else {
        return false;
    };
    graph.resolve(module, &dump.func) == Some(Binding::Intrinsic(Intrinsic::TomlDumps))
        && dump.arguments.keywords.is_empty() && dictionary.items.len() == fields.len()
        && dictionary.items.iter().zip(fields).all(|(item, field)| {
            matches!(&item.key, Some(ast::Expr::StringLiteral(name)) if name.value.to_str() == field.name)
                && matches!(&item.value, ast::Expr::Attribute(attr) if attr.attr.as_str() == field.name
                    && matches!(&*attr.value, ast::Expr::Name(name) if name.id == receiver.name().as_str()))
        })
}
