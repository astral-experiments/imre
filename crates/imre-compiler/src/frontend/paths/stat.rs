//! Bind Path.stat and Path.lstat before applying the OS pathname and truth converters.

use super::*;

impl Checker<'_> {
    pub(super) fn path_stat(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let lstat = name == "lstat";
        let mut follow = None;
        let mut error = None;
        for keyword in &call.arguments.keywords {
            let key = keyword.arg.as_ref().unwrap().as_str();
            if !lstat && key == "follow_symlinks" {
                follow = Some(&keyword.value);
            } else if key == "self" {
                self.graph
                    .checked_repeated_keyword(self.module, keyword.range());
                error.get_or_insert_with(|| {
                    format!("Path.{name}() got multiple values for argument 'self'")
                });
            } else {
                self.graph
                    .checked_keywords(self.module, std::iter::once(keyword.range()));
                error.get_or_insert_with(|| {
                    format!("Path.{name}() got an unexpected keyword argument '{key}'")
                });
            }
        }
        if !call.arguments.args.is_empty() {
            for arg in &call.arguments.args {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
            let count = call.arguments.args.len() + 1;
            let extra = if follow.is_some() {
                " positional arguments (and 1 keyword-only argument)"
            } else {
                ""
            };
            error.get_or_insert_with(|| {
                format!("Path.{name}() takes 1 positional argument but {count}{extra} were given")
            });
        }
        let ty = self.types.stat_result();
        let mut scope = calls::ArgumentScope::default();
        let receiver = scope.hold(self, receiver);
        if let Some(message) = error {
            return Ok(scope.finish(self.sys_type_error(call, ty, message)?));
        }
        let flag = if let Some(expression) = follow {
            let value = self.expr(expression)?;
            if value.ty == Type::Bool {
                value
            } else {
                let value = scope.hold(self, value);
                scope.hold(
                    self,
                    operation(
                        PathOperation::CheckStatPath,
                        Type::None,
                        vec![receiver.clone()],
                    ),
                );
                self.truth_value(value)
            }
        } else {
            boolean(!lstat)
        };
        Ok(scope.finish(operation(
            PathOperation::Stat { lstat },
            ty,
            vec![receiver, flag],
        )))
    }
}
