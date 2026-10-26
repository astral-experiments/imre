//! One-based nominal tags share a process-constant table of pooled text roots.

use super::*;

/// Fold finite tag producers into one exhaustive dispatch without duplicating
/// unbounded bodies or recursively forcing nested dispatches to inline.
pub(super) fn inline_dispatch(module: &Module, effects: &effects::ExceptionEffects) -> Vec<bool> {
    let mut sites = vec![0usize; module.functions.len()];
    for function in module.functions.iter().chain([&module.entry]) {
        crate::ir::walk_expressions(&function.body, &mut |expression| {
            if let ExprKind::Call(target, args) = &expression.kind
                && args.exhaustive_enum_match
            {
                sites[*target] += 1;
            }
        });
    }
    module.functions.iter().enumerate().map(|(id, function)| {
        if sites[id] != 1 || !matches!(function.returns, Type::StrEnum(_)) || effects.may_raise(id) {
            return false;
        }
        let mut nodes = 0;
        let mut nested = false;
        crate::ir::walk_expressions(&function.body, &mut |expression| {
            nodes += 1;
            nested |= matches!(&expression.kind, ExprKind::Call(_, args) if args.exhaustive_enum_match);
        });
        nodes <= 192 && !nested
    }).collect()
}

impl Emitter<'_, '_> {
    pub(super) fn prepare_enum_members(&mut self) {
        let module = self.module;
        for class in &module.class_metadata {
            for (name, value) in &class.enum_members {
                let fields = [&class.name, name, value]
                    .map(|text| self.intern_literal(text.as_bytes(), Type::Str));
                self.enum_members.push(fields);
            }
        }
    }

    pub(super) fn enum_tag(&self, class: usize, member: usize) -> usize {
        1 + member
            + self.module.class_metadata[..class]
                .iter()
                .map(|class| class.enum_members.len())
                .sum::<usize>()
    }

    pub(super) fn enum_table(&mut self) {
        if self.enum_members.is_empty() {
            return;
        }
        let styles = self
            .module
            .class_metadata
            .iter()
            .flat_map(|class| {
                std::iter::repeat_n(
                    if class.plain_enum { "i8 1" } else { "i8 0" },
                    class.enum_members.len(),
                )
            })
            .collect::<Vec<_>>();
        writeln!(
            self.output.global(
                self.output.entry,
                "enum_display_styles",
                "constant",
                format_args!("[{} x i8]", styles.len())
            ),
            "[{}]",
            styles.join(", ")
        )
        .unwrap();
        let rows = self
            .enum_members
            .iter()
            .map(|row| {
                let [class, name, value] = row.map(|id| self.output.string_name(id, "literal"));
                format!("[3 x ptr] [ptr @{class}, ptr @{name}, ptr @{value}]")
            })
            .collect::<Vec<_>>();
        writeln!(
            self.output.global(
                self.output.entry,
                "str_enum_members",
                "constant",
                format_args!("[{} x [3 x ptr]]", rows.len())
            ),
            "[{}]",
            rows.join(", ")
        )
        .unwrap();
    }

    pub(super) fn enum_text(&mut self, tag: &Value, name: bool) -> Value {
        let index = self.instruction(format!("sub nuw i64 {}, 1", tag.operand));
        let field = if name { 1 } else { 2 };
        let table = self.output.global_ref("str_enum_members".to_owned());
        let ptr = self.instruction(format!("getelementptr inbounds [{} x [3 x ptr]], ptr @{table}, i64 0, i64 {index}, i64 {field}", self.enum_members.len()));
        let root = self.instruction(format!("load ptr, ptr {ptr}"));
        let result = Value {
            ty: Type::Str,
            operand: self.instruction(format!("load ptr, ptr {root}")),
            small_integer: false,
        };
        self.retain(&result);
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tags_and_nullable_tags_pass_as_scalar_words() {
        let llvm = crate::compile(
            r#"
from enum import StrEnum
class S(StrEnum):
    ON = "on"
    OFF = "off"
def same(a: S, b: S | None) -> bool:
    return a is b
def label(s: S) -> str:
    return s.name
print(same(S.ON, S.OFF))
print(label(S.ON))
"#,
            &crate::CompileOptions::default(),
        )
        .unwrap();
        assert!(llvm.contains("@str_enum_members = private constant [2 x [3 x ptr]]"));
        assert!(llvm.contains("icmp eq i64"));
        assert!(llvm.contains("getelementptr inbounds [2 x [3 x ptr]]"));
        assert!(
            !llvm
                .lines()
                .any(|line| line.contains("call ptr @imre_record_try_new("))
        );
    }

    #[test]
    fn only_the_single_infallible_exhaustive_callsite_is_inlined() {
        let definitions = r#"
from enum import StrEnum
class Side(StrEnum):
    FIRST = "a"
    SECOND = "b"
def pick(check: bool) -> Side:
    if check:
        return Side.FIRST
    return Side.SECOND
"#;
        let dispatch = r#"
def show(check: bool) -> str:
    match pick(check):
        case Side.FIRST:
            return "a"
        case Side.SECOND:
            return "b"
"#;
        let sites = |source: String| {
            crate::compile(&source, &crate::CompileOptions::default())
                .unwrap()
                .lines()
                .filter(|line| line.contains("call i64 @fn_0(") && line.ends_with(" alwaysinline"))
                .count()
        };
        assert_eq!(
            sites(format!(
                "{definitions}{dispatch}\nprint(show(True))\nprint(pick(False))"
            )),
            1
        );
        let second = dispatch.replace("def show(", "def again(");
        let fallible = definitions.replace("return Side.SECOND", "raise ValueError(\"no side\")");
        let partial = dispatch.replace("case Side.SECOND:", "case Side.SECOND if check:");
        assert_eq!(sites(format!("{definitions}{dispatch}{second}")), 0);
        assert_eq!(sites(format!("{fallible}{dispatch}")), 0);
        assert_eq!(
            sites(format!("{definitions}{partial}    return \"other\"\n")),
            0
        );
    }
}
