use super::*;

/// Register an existing checked function with its original frame positions.
/// Native analysis has already verified the operation sequence after specialization.
pub(crate) fn place_fixed_function(
    mut function: ir::Function,
    source: FixedBodySource,
) -> RegisteredFunction {
    function.returns = Type::PythonObject;
    RegisteredFunction {
        function,
        local_names: source.local_names,
        fixed_sites: source.positions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_sites_survive_only_unchanged_checked_operations() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: str) -> str:\n        return value\n",
            "def append(left: str, right: str) -> str:\n",
            "    return left + right\n",
            "print(append('a', 'b'))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected fixed checked operations")
        };
        let mut module = fixed.ir.0;
        let mut sites = fixed.frame_sources;
        let mut changed = sites.clone();
        crate::llvm::analyze_program(&module, &mut sites).unwrap();
        let (_, source) = sites.remove(&0).unwrap();
        let function = module.functions[0].clone();
        let body = function.body.as_ptr();
        let positions = source.positions.as_ptr();
        let placed = place_fixed_function(function, source);
        assert_eq!(placed.fixed_sites.as_ptr(), positions);
        assert_eq!(placed.function.body.as_ptr(), body);
        assert_eq!(placed.local_names[&0].name, "left");
        assert_eq!(placed.local_names[&1].name, "right");
        let spellings: Vec<_> = placed.fixed_sites.iter().map(|r| &text[*r]).collect();
        assert_eq!(
            spellings,
            ["return left + right", "left + right", "left", "right"]
        );

        // An optimizer can change a slot without changing the operation count or type.
        let [Statement::Return(result)] = module.functions[0].body.as_mut_slice() else {
            panic!()
        };
        let ExprKind::Binary(_, _, right) = &mut result.kind else {
            panic!()
        };
        right.kind = ExprKind::Local(0);
        crate::llvm::analyze_program(&module, &mut changed).unwrap();
        assert!(!changed.contains_key(&0));
    }
}
