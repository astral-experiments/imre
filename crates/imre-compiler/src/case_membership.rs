//! Use known set keys to skip string transformations and empty character filters.
use crate::ir::{self, Builtin, Expr, ExprKind, Function, Module, Statement, TextOperation, Type};
use crate::types::Types;

/// The guard and the ordinary membership test may both read this receiver.
fn stable_string_set(types: &Types, value: &Expr) -> bool {
    types.set_element(value.ty) == Some(Type::Str)
        && (matches!(value.kind, ExprKind::Local(_))
            || matches!(&value.kind, ExprKind::Builtin(Builtin::GlobalGet(_), args) if args.is_empty()))
}

/// Finds the operand of an exact native string case conversion.
fn case_input(value: &mut Expr) -> Option<&mut Expr> {
    if let ExprKind::Builtin(Builtin::Text(TextOperation::Lower | TextOperation::Upper), args) =
        &mut value.kind
        && let [text] = args.as_mut_slice()
        && text.ty == Type::Str
    {
        Some(text)
    } else {
        None
    }
}

/// Summarize literal key prefixes. An empty key can match after truncating any
/// input, so it disables prefix rejection.
fn literal_prefix_mask(values: &[Expr]) -> Option<ir::SetCasePrefix> {
    let mut masks = [u64::MAX; 2];
    const MAX_PREFIX_TAGS: usize = 8;
    let mut tags = Vec::new();
    let mut hashes = Some(0_u64);
    for value in values {
        let ExprKind::Str(key) = &value.kind else {
            return None;
        };
        let first = *key.as_bytes().first()?;
        hashes = hashes.and_then(|mask| {
            let bytes = key.as_bytes();
            let bit = imre_ast::strings::ascii_case_prefix_bit(bytes)?;
            if tags.len() <= MAX_PREFIX_TAGS {
                // Punctuation sharing the ASCII case bit may keep an extra candidate.
                let tag = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]) | 0x20_20_20;
                if !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
            Some(mask | bit)
        });
        if first.is_ascii() {
            for byte in [first.to_ascii_uppercase(), first.to_ascii_lowercase()] {
                masks[usize::from(byte / 64)] &= !(1_u64 << (byte % 64));
            }
        }
    }
    // A small switch compares the prefix values directly. For larger sets, use a
    // hash mask only when at least three quarters of its buckets reject.
    Some(match hashes {
        Some(mask) if (1..=MAX_PREFIX_TAGS).contains(&tags.len()) => {
            ir::SetCasePrefix::ThreeByteValues { mask, tags }
        }
        Some(mask) if (1..=16).contains(&mask.count_ones()) => {
            ir::SetCasePrefix::ThreeByteHashes(mask)
        }
        _ => ir::SetCasePrefix::ExcludedBytes(masks),
    })
}

/// Summaries of exact keys whose set is only used by read-only operations.
#[derive(Clone, Default)]
struct LiteralSet {
    prefix: Option<ir::SetCasePrefix>,
    ascii_keys: Option<[u64; 2]>,
}

/// A single UTF-8 byte denotes an ASCII character. Longer keys need the ordinary filter.
fn ascii_character_keys(values: &[Expr]) -> Option<[u64; 2]> {
    let mut bits = [0; 2];
    for value in values {
        let ExprKind::Str(key) = &value.kind else {
            return None;
        };
        let &[byte] = key.as_bytes() else {
            return None;
        };
        bits[usize::from(byte / 64)] |= 1_u64 << (byte % 64);
    }
    Some(bits)
}

/// A single literal initializer describes the set only if every read is a lookup
/// or character filter. Assignments, calls, and aliases disqualify it.
fn constant_sets(module: &Module) -> Vec<LiteralSet> {
    #[derive(Clone, Default)]
    struct Uses {
        writes: usize,
        reads: usize,
        read_only: usize,
        literal: LiteralSet,
    }
    let mut globals = vec![Uses::default(); module.globals.len()];
    for function in module.functions.iter().chain([&module.entry]) {
        ir::walk_expressions(&function.body, &mut |expr| {
            let ExprKind::Builtin(operation, args) = &expr.kind else {
                return;
            };
            match (operation, args.as_slice()) {
                (Builtin::GlobalSet(slot), [value]) => {
                    let uses = &mut globals[*slot];
                    uses.writes += 1;
                    if let ExprKind::Set {
                        element: Type::Str,
                        values,
                    } = &value.kind
                    {
                        uses.literal = LiteralSet {
                            prefix: literal_prefix_mask(values),
                            ascii_keys: ascii_character_keys(values),
                        };
                    }
                }
                (Builtin::GlobalGet(slot), []) => globals[*slot].reads += 1,
                (Builtin::SetContains | Builtin::FilterCharacterSet(_), [_, set]) => {
                    if let ExprKind::Builtin(Builtin::GlobalGet(slot), _) = set.kind {
                        globals[slot].read_only += 1;
                    }
                }
                _ => {}
            }
        });
    }
    globals
        .into_iter()
        .map(|uses| {
            if uses.writes == 1 && uses.reads == uses.read_only {
                uses.literal
            } else {
                LiteralSet::default()
            }
        })
        .collect()
}

/// Find the local whose prefix this expression returns. Literal arguments keep the
/// skipped operations free of callbacks and their separators valid on every path.
fn prefix_local(value: &Expr) -> Option<usize> {
    let ExprKind::Builtin(builtin, args) = &value.kind else {
        return None;
    };
    let source = match (builtin, args.as_slice()) {
        (Builtin::RStrip, [source]) => source,
        (Builtin::Text(TextOperation::Strip(2)), [source, chars])
            if matches!(&chars.kind, ExprKind::Absent(value) if matches!(value.kind, ExprKind::None))
                || matches!(&chars.kind, ExprKind::Present(value) if matches!(value.kind, ExprKind::Str(_))) =>
        {
            source
        }
        (Builtin::ListGet(_), [split, index]) => {
            let (false, [source, separator]) = split.selected_split_args(index)? else {
                return None;
            };
            if !matches!(&separator.kind, ExprKind::Str(text) if !text.is_empty()) {
                return None;
            }
            source
        }
        _ => return None,
    };
    match source.kind {
        ExprKind::Local(slot) => Some(slot),
        _ => prefix_local(source),
    }
}

/// Tests the original string before computing a prefix when the lookup set is constant.
fn prefix_check(set: &Expr, source: &Expr, sets: &[LiteralSet]) -> Option<Expr> {
    let ExprKind::Builtin(Builtin::GlobalGet(slot), _) = set.kind else {
        return None;
    };
    Some(possible(
        Builtin::SetCasePrefixPossible(sets[slot].prefix.clone()?),
        set,
        prefix_local(source)?,
    ))
}

/// Builds a guard for the stable set and an already available string.
fn possible(probe: Builtin, set: &Expr, text: usize) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Builtin(probe, vec![set.clone(), Expr::local(text, Type::Str)]),
    }
}

/// The miss result must be independent of the skipped assignment.
fn independent(value: &Expr, target: usize) -> bool {
    match &value.kind {
        ExprKind::None
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_) => true,
        ExprKind::Local(slot) => *slot != target,
        ExprKind::Present(value) | ExprKind::Absent(value) => independent(value, target),
        _ => false,
    }
}

/// A normalizer may return the converted spelling on a hit. On a definite
/// miss, its final return bypasses the assignment without exposing the local.
fn guard_return(function: &mut Function, types: &Types, sets: &[LiteralSet]) {
    let [
        ..,
        assignment,
        Statement::If(test, _, otherwise),
        Statement::Return(miss),
    ] = function.body.as_mut_slice()
    else {
        return;
    };
    let Statement::Assign(target, converted) = assignment else {
        return;
    };
    let ExprKind::Builtin(Builtin::SetContains, args) = &test.kind else {
        return;
    };
    let [key, set] = args.as_slice() else { return };
    if !otherwise.is_empty()
        || !matches!(key.kind, ExprKind::Local(slot) if slot == *target)
        || !stable_string_set(types, set)
        || !independent(miss, *target)
    {
        return;
    }
    let Some(input) = case_input(converted) else {
        return;
    };
    let temp = function.locals.len();
    function.locals.push(Type::Str);
    let source = std::mem::replace(input, Expr::local(temp, Type::Str));
    let miss_unless = |check| {
        let skip = Expr::not(check);
        Statement::If(skip, vec![Statement::Return(miss.clone())], vec![])
    };
    let mut body = Vec::new();
    if let Some(check) = prefix_check(set, &source, sets) {
        body.push(miss_unless(check));
    }
    body.push(Statement::Assign(temp, source));
    body.push(miss_unless(possible(
        Builtin::SetCaseLengthPossible,
        set,
        temp,
    )));
    body.push(std::mem::replace(assignment, Statement::Clear(temp)));
    *assignment = Statement::Scope {
        body,
        clear: vec![temp],
    };
}

/// Guard a boolean membership expression with a definite-miss test.
fn when_possible(check: Expr, value: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Conditional(
            Box::new(check),
            Box::new(value),
            Box::new(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(false),
            }),
        ),
    }
}

/// Guard inline membership and normalize-and-return functions. A private slot
/// evaluates the source exactly once, even when obtaining it has side effects.
pub(crate) fn specialize(module: &mut Module) {
    let sets = constant_sets(module);
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        let original_locals = function.locals.len();
        let types = &module.types;
        guard_return(function, types, &sets);
        ir::walk_expressions_mut(&mut function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::FilterCharacterSet(filter), args) = &mut expr.kind
                && let [_, keys] = args.as_slice()
                && let ExprKind::Builtin(Builtin::GlobalGet(slot), _) = keys.kind
            {
                filter.ascii_keys = sets[slot].ascii_keys;
            }
            let ExprKind::Builtin(Builtin::SetContains, args) = &mut expr.kind else {
                return;
            };
            let [key, set] = args.as_mut_slice() else {
                return;
            };
            if !stable_string_set(types, set) {
                return;
            }
            let Some(input) = case_input(key) else { return };
            // The preorder walk also visits the ordinary conversion in our fallback.
            if matches!(input.kind, ExprKind::Local(slot) if slot >= original_locals) {
                return;
            }
            let temp = function.locals.len();
            function.locals.push(Type::Str);
            let source = std::mem::replace(input, Expr::local(temp, Type::Str));
            let prefix = prefix_check(set, &source, &sets);
            let check = possible(Builtin::SetCaseLengthPossible, set, temp);
            let ordinary = std::mem::replace(expr, Expr::none());
            let guarded = Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(temp, source)],
                    value: Box::new(when_possible(check, ordinary)),
                    clear: vec![temp],
                },
            };
            *expr = match prefix {
                Some(check) => when_possible(check, guarded),
                None => guarded,
            };
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts guards in the lowered program, including the early return on a definite miss.
    fn guards(source: &str) -> (usize, usize) {
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        specialize(&mut module);
        let mut count = (0, 0);
        for function in module.functions.iter().chain([&module.entry]) {
            ir::walk_expressions(&function.body, &mut |value| match value.kind {
                ExprKind::Builtin(Builtin::SetCaseLengthPossible, _) => count.0 += 1,
                ExprKind::Builtin(Builtin::SetCasePrefixPossible(_), _) => count.1 += 1,
                _ => {}
            });
        }
        count
    }

    #[test]
    fn three_byte_prefixes_require_three_ascii_bytes_in_every_key() {
        let prefixes = |keys: &[&str]| {
            let values: Vec<_> = keys
                .iter()
                .map(|key| Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str((*key).to_owned()),
                })
                .collect();
            literal_prefix_mask(&values)
        };
        let abc = imre_ast::strings::ascii_case_prefix_bit(b"ABC").unwrap();
        assert!(
            matches!(prefixes(&["Abc", "ABCD"]), Some(ir::SetCasePrefix::ThreeByteValues { mask, tags }) if mask == abc && tags == [0x61_62_63])
        );
        assert!(matches!(
            prefixes(&[
                "aaa", "aab", "aac", "aad", "aae", "aaf", "aag", "aah", "aai"
            ]),
            Some(ir::SetCasePrefix::ThreeByteHashes(_))
        ));
        for keys in [["abc", "ab"], ["abc", "ßss"]] {
            assert!(matches!(
                prefixes(&keys),
                Some(ir::SetCasePrefix::ExcludedBytes(_))
            ));
        }
        assert!(prefixes(&["abc", ""]).is_none());
    }

    #[test]
    fn guards_case_membership_with_an_independent_miss() {
        assert_eq!(
            guards(
                "def check(s: str, names: set[str]) -> bool:\n    return s.lower() in names\nprint(check('A', {'a'}))\n"
            ),
            (1, 0)
        );
        assert_eq!(
            guards(
                "def find(s: str, names: set[str]) -> str | None:\n    key = s.upper()\n    if key in names:\n        return key\n    return None\nfind('a', {'A'})\n"
            ),
            (1, 0)
        );
    }

    #[test]
    fn preserves_a_converted_miss_and_computed_set_receiver() {
        assert_eq!(
            guards(
                "def find(s: str, names: set[str]) -> str:\n    key = s.upper()\n    if key in names:\n        return s\n    return key\nprint(find('a', {'A'}))\n"
            ),
            (0, 0)
        );
        assert_eq!(
            guards(
                "def names() -> set[str]:\n    return {'x'}\ndef check(s: str) -> bool:\n    return s.upper() in names()\nprint(check('x'))\n"
            ),
            (0, 0)
        );
    }

    #[test]
    fn probes_only_infallible_prefix_operations() {
        for (key, prefix) in [
            ("s.rstrip().upper()", 1),
            ("s.split('.', 1)[0].rstrip(' ').lower()", 1),
            ("s.lstrip().upper()", 0),
            ("s.split('', 1)[0].lower()", 0),
            ("s.split(sep, 1)[0].upper()", 0),
            ("s.rsplit('.', 1)[-1].lower()", 0),
        ] {
            let source = format!(
                "NAMES = {{'ABC'}}\ndef check(s: str, sep: str) -> bool:\n    return {key} in NAMES\nprint(check('a', '.'))\n"
            );
            assert_eq!(guards(&source), (1, prefix), "{key}");
        }
    }

    #[test]
    fn preserves_mutable_or_empty_keys() {
        for setup in [
            "NAMES.add('xyz')",
            "alias = NAMES\nalias.add('xyz')",
            "NAMES.add('')",
        ] {
            let source = format!(
                "NAMES = {{'ABC'}}\ndef check(s: str) -> bool:\n    return s.rstrip().upper() in NAMES\n{setup}\nprint(check('xyz'))\n"
            );
            assert_eq!(guards(&source), (1, 0), "{setup}");
        }
        assert_eq!(
            guards(
                "NAMES = {'', 'ABC'}\ndef check(s: str) -> bool:\n    return s.rstrip().upper() in NAMES\nprint(check(' '))\n"
            ),
            (1, 0)
        );
    }
}
