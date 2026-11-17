//! Bound Python syntax depth before recursive parser indexing or compilation.

use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{Mode, lexer::lex};

/// Reject the aggregate expression and control-flow depth used by compiler indexing.
pub fn check(source: &str) -> Result<(), &'static str> {
    check_with_policy(source, Policy::Compiler)
}

/// Bound recursive expression and primary spines before runtime `compile` parses source.
///
/// Runtime compilation does not perform the compiler's recursive control-flow indexing, so
/// statement-suite depth must not contribute to this refusal. The compiler may omit a linked
/// body only when this same check guarantees that runtime `compile` will raise instead.
pub fn check_runtime_compile(source: &str) -> Result<(), &'static str> {
    check_with_policy(source, Policy::RuntimeCompile)
}

#[derive(Clone, Copy)]
enum Policy {
    Compiler,
    RuntimeCompile,
}

fn check_with_policy(source: &str, policy: Policy) -> Result<(), &'static str> {
    const LIMIT: usize = 96;
    if source.len() < LIMIT {
        return Ok(());
    }

    #[derive(Default)]
    struct Group {
        operators: usize,
        primary_depth: usize,
        longest_primary: usize,
        longest_item: usize,
        enclosing_operators: usize,
        longest_boolean_operand: usize,
        boolean_operators: usize,
        open_lambda_parameters: usize,
        // Replacement fields are siblings in an f-string or template string.
        interpolated_string: bool,
        // Commas in a comprehension target belong to one assignment target.
        keep_commas: bool,
    }

    impl Group {
        fn operand_depth(&self) -> usize {
            self.operators + self.longest_primary.max(self.primary_depth)
        }

        fn depth(&self, policy: Policy) -> usize {
            let operand = self.operand_depth();
            // A Boolean expression has at most an outer "or" and an inner "and".
            // Ruff stores the operands of each operator as siblings.
            let boolean = if self.boolean_operators == 0 {
                operand
            } else {
                2 + self.longest_boolean_operand.max(operand)
            };
            let depth = self.longest_item.max(self.enclosing_operators + boolean);
            // ty tracks a flow branch at each short circuit, even for a flat AST.
            match policy {
                Policy::Compiler => depth.max(self.boolean_operators),
                Policy::RuntimeCompile => depth,
            }
        }

        fn next_item(&mut self, policy: Policy) {
            self.longest_item = self.depth(policy);
            self.enclosing_operators = 0;
            self.longest_boolean_operand = 0;
            self.boolean_operators = 0;
            self.operators = 0;
            self.primary_depth = 0;
            self.longest_primary = 0;
        }
    }

    use TokenKind::*;
    let mut lexer = lex(source, Mode::Module);
    let mut parents = Vec::new();
    let mut group = Group::default();
    let mut indents = Vec::new();
    let mut elif_depth = 0;
    let mut statement_start = true;
    loop {
        let token = lexer.next_token();
        if statement_start
            && !matches!(
                token,
                Indent | Dedent | Newline | Comment | NonLogicalNewline
            )
        {
            // Each elif adds a nested false branch. Its suite inherits that
            // depth, while a new statement starts a fresh chain at this indent.
            match token {
                Elif => elif_depth += 1,
                Else => {}
                _ => elif_depth = indents.last().copied().unwrap_or(0),
            }
            statement_start = false;
        }
        match token {
            EndOfFile => return Ok(()),
            Indent => indents.push(elif_depth),
            Dedent => elif_depth = indents.pop().unwrap_or(0),
            Newline | Semi if parents.is_empty() => {
                group = Group::default();
                statement_start = token == Newline;
            }
            Lpar | Lsqb | Lbrace | FStringStart | TStringStart => {
                if token == Lbrace && group.interpolated_string {
                    group.next_item(policy);
                }
                group.primary_depth += 1;
                parents.push(std::mem::take(&mut group));
                group.interpolated_string = matches!(token, FStringStart | TStringStart);
            }
            Rpar | Rsqb | Rbrace | FStringEnd | TStringEnd => {
                if let Some(mut parent) = parents.pop() {
                    parent.primary_depth = parent.primary_depth.max(1 + group.depth(policy));
                    group = parent;
                }
            }
            Comma if !group.keep_commas && group.open_lambda_parameters == 0 => {
                group.next_item(policy);
            }
            And | Or => {
                let depth = group.operand_depth();
                group.longest_boolean_operand = group.longest_boolean_operand.max(depth);
                group.boolean_operators += 1;
                group.operators = 0;
                group.primary_depth = 0;
                group.longest_primary = 0;
            }
            Dot => group.primary_depth += 1,
            kind if kind.as_binary_operator().is_some() => {
                // In f() + g(), the calls are siblings. Count every operator on
                // the expression spine and the deepest of its primary operands.
                group.longest_primary = group.longest_primary.max(group.primary_depth);
                group.primary_depth = 0;
                group.operators += 1;
            }
            // These expressions can enclose Boolean operands across the entire item.
            Lambda => {
                group.open_lambda_parameters += 1;
                group.enclosing_operators += 1;
            }
            Colon if group.open_lambda_parameters > 0 => {
                group.open_lambda_parameters -= 1;
                group.operators += 1;
            }
            For => {
                group.keep_commas = true;
                group.enclosing_operators += 1;
            }
            If | Else | Yield | ColonEqual => group.enclosing_operators += 1,
            Identifier | Int | Float | Complex | String | True | False | None | Ellipsis
            | FStringMiddle | TStringMiddle | Comment | NonLogicalNewline => continue,
            _ => group.operators += 1,
        }
        let expression_depth = parents.len() + group.depth(policy);
        let depth = match policy {
            Policy::Compiler => indents.len() + elif_depth + expression_depth,
            Policy::RuntimeCompile => expression_depth,
        };
        if depth > LIMIT {
            return Err("Imre: source exceeds the syntax complexity limit");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{check, check_runtime_compile};

    #[test]
    fn accepts_shallow_and_rejects_deep_operator_spines() {
        let shallow = format!("value = 1{}", "+1".repeat(32));
        let deep = format!("value = 1{}", "+1".repeat(256));
        assert!(check(&shallow).is_ok());
        assert!(check_runtime_compile(&shallow).is_ok());
        assert!(check(&deep).is_err());
        assert!(check_runtime_compile(&deep).is_err());
    }

    #[test]
    fn runtime_compile_accepts_suites_beyond_compiler_indexing_limit() {
        let mut source = (0..96)
            .map(|depth| format!("{}if True:\n", " ".repeat(depth)))
            .collect::<String>();
        source.push_str(&format!("{}result = 7\n", " ".repeat(96)));
        assert!(check(&source).is_err());
        assert!(check_runtime_compile(&source).is_ok());
    }

    #[test]
    fn runtime_compile_does_not_count_flat_boolean_flow_branches() {
        let source = format!("value = True{}", " and True".repeat(256));
        assert!(check(&source).is_err());
        assert!(check_runtime_compile(&source).is_ok());
    }
}
