use imre_compiler::{CompileOptions, check_file, compile_file};

#[test]
fn inferred_storage_follows_the_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.py");
    for imports in ["", "import unittest\n"] {
        for body in [
            "values = [1, 2]; values[:] = [3]; print(values)",
            "a = []; a.append(1); a.append('str'); print(a)",
            "x = y = [1]; x.extend(['text']); print(y)",
            "def change(xs, flag):\n    if flag: xs.append(True)\na: list[int] = []\nchange(a, False)\nchange([1], True)",
            "x = [1]; alias = x; alias += [True]; print(x is alias, x)",
            "x: list[int] = [1]; alias = x; alias += x; print(x)",
            "a = [1]; b: list[int] = [2]\nif False: b = a\na.append('text')",
            "s = set([1]); s.add('x'); print(s)",
            "a = [x for x in range(3)]; a[0] = 'x'; print(a)",
            "d = {1: 2, 'key': 'val'}; print(d, {1, 'two'})",
            "d = {}; d['a'] = 1; d.setdefault('b', 'other'); print(d)",
            "def take(x: list[int]) -> None: pass\na = [1]\nif False: a.append('text')\ntake(a)",
            "def take(x: list[str]) -> None: pass\na = []; other = a; other.append('ok'); take(a)",
            "values = [False]; alias = values; alias[0] = b'x'; print(values)",
            "d = {'a': 1}; ref = d; ref[2] = 'text'; print(d)",
            "s = {1}; ref = s; ref.add('text'); print(s)",
            "class Label:\n    def __repr__(self): return 'label'\nprint({Label(): 1})",
            "def f() -> None:\n    fixed: list[int] = [1]\n    alias = fixed\n    alias = [2]\n    alias.append('text')\n    print(alias, fixed)\nf()",
        ] {
            std::fs::write(&file, format!("{imports}{body}\n")).unwrap();
            check_file(&file, None).unwrap_or_else(|e| panic!("{body}\n{e}"));
        }
    }
    std::fs::write(&file, "a = []; a.append(1); a.append(2); print(a)\n").unwrap();
    let program = compile_file(&file, None, &CompileOptions::default()).unwrap();
    assert!(!program.source_inputs.boxed_imports);
}

#[test]
fn mutable_contracts_survive_aliases_and_boxed_writes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.py");
    for imports in ["", "import unittest\n"] {
        for body in [
            "values: list[int] = [1, 2]; values[:] = [True]",
            "a: list[int] = []; b = a; b.append('wrong')",
            "def change(xs): xs.append(True)\na: list[int] = []\nchange(a)",
            "a: list[int] = [1]; b = a; b.extend(['wrong'])",
            "x: list[int] = [1]; x += [True]; print(x)",
            "x: list[int] = [1]; alias = x; alias += [True]; print(x)",
            "x: list[int] = [1]; alias = x\nfor _ in range(2): alias += [True]",
            "x: list[int] = [1]; x += (True for _ in range(2))",
            "x: list[int] = [1]; rhs = [1, True]; x += rhs",
            "x = [1]; alias: list[int] = x; x += (True,)",
            "x: list[int] = [1]; alias = x; alias += [2]; alias.append(True)",
            "def take(x: list[int]) -> None: pass\nx = [1]; alias = x; alias += ['s']; take(x)",
            "def take(x: list[int]) -> None: pass\ng = take; a = [1]; a.append('other'); g([True])",
            "a = [1]; b: list[int] = a; a.append('wrong')",
            "d: dict[str, int] = {}; other = d; other['a'] = False",
            "s: set[int] = {1}; other = s; other.add('wrong')",
            "def take(x: list[int]) -> None: pass\na = [1]; b = a; b.append('text'); take(a)",
            "def take(x: list[int]) -> None: pass\ng = take; a = [1]; b = a; b[0] = False; g(a)",
            "def take(x: dict[str, int]) -> None: pass\na = {}; b = a; b['k'] = 'bad'; take(a)",
            "def take(x: int) -> str: return 'ok'\na = [1]; a.append(take('wrong'))",
        ] {
            std::fs::write(&file, format!("{imports}{body}\n")).unwrap();
            let e = check_file(&file, None).expect_err(body);
            assert!(
                e.contains("expected")
                    || e.contains("invalid-argument-type")
                    || e.contains("invalid-assignment"),
                "{e}"
            );
        }
    }
}

#[test]
fn excluded_body_cannot_constrain_an_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.py");
    std::fs::write(&file, "a = []\ndef unused() -> None: a.append('s')\ndef take(x: list[int]) -> None: pass\na.append(1); take(a)\n").unwrap();
    imre_compiler::check_file_with_options(
        &file,
        None,
        &CompileOptions {
            reachable_only: true,
            ..Default::default()
        },
    )
    .unwrap();
}
