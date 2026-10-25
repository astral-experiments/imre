"""Locate the release guard behind each nonpassing OS reference occurrence."""
import ast

# These skips happen inside the method or a fixture, not while the test is
# decorated. The exact frozen source is checked before their line is recorded.
RUNTIME = {
    "test.test_os.TermsizeTests.test_does_not_crash": (
        "os.get_terminal_size() raised OSError and (sys.platform == 'win32' or e.errno in known_errnos)",
        "stdin/stdout are not terminals; the original terminal query failed", "self.skipTest"),
    "test.test_os.TermsizeTests.test_stty_match": (
        "stty size raises FileNotFoundError, subprocess.CalledProcessError, or PermissionError",
        "stty has no terminal in the headless reference fixture", "self.skipTest"),
    "test.test_os.UtimeTests.test_large_time": (
        "self.get_file_system(self.dirname) != 'NTFS'",
        "the POSIX temporary filesystem is not identified as NTFS", "self.skipTest"),
    "test.test_posix.PosixGroupsTester.test_initgroups": (
        "posix.getuid() != 0 in PosixGroupsTester.setUp", "the reference user is unprivileged", "unittest.SkipTest"),
    "test.test_posix.PosixGroupsTester.test_setgroups": (
        "posix.getuid() != 0 in PosixGroupsTester.setUp", "the reference user is unprivileged", "unittest.SkipTest"),
}


def skip_guard(name, reason, tree):
    classname, method = name.rsplit(".", 2)[1:]
    cls, = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == classname]
    methods = {n.name: n for n in cls.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))}
    node = methods[method]
    if name in RUNTIME:
        condition, configuration, called = RUNTIME[name]
        scope = methods["setUp"] if classname == "PosixGroupsTester" else node
        call, = [n for n in ast.walk(scope) if isinstance(n, ast.Call) and ast.unparse(n.func) == called
                 and n.args and isinstance(n.args[0], ast.Constant) and n.args[0].value == reason]
        return condition, configuration, call.lineno

    aliases = {target.id: child.value for parent in (tree, cls) for child in parent.body
               if isinstance(child, ast.Assign) for target in child.targets if isinstance(target, ast.Name)}
    for decorator in [*cls.decorator_list, *node.decorator_list]:
        guard = aliases.get(decorator.id, decorator) if isinstance(decorator, ast.Name) else decorator
        if not isinstance(guard, ast.Call):
            continue
        function = ast.unparse(guard.func)
        if function == "support.bigmemtest" and reason == "not enough memory: 2.0G minimum needed":
            options = {kw.arg: ast.unparse(kw.value) for kw in guard.keywords}
            if options == {"size": "INT_MAX + 10", "memuse": "1", "dry_run": "False"}:
                return ("bigmemtest: (real_max_memuse or not dry_run) and real_max_memuse < maxsize * memuse",
                        "reference regrtest did not enable the 2 GiB memory resource", guard.lineno)
        if not guard.args or not isinstance(guard.args[-1], ast.Constant) or guard.args[-1].value != reason:
            continue
        if function == "unittest.skip":
            return ("unconditional upstream unittest.skip", "upstream marks this fixture skipped", guard.lineno)
        if function not in {"unittest.skipIf", "unittest.skipUnless"}:
            continue
        operand = guard.args[0]
        # Record the actual release expression behind local capability names.
        if isinstance(operand, ast.Name) and operand.id in aliases:
            operand = aliases[operand.id]
        pred = ast.unparse(operand)
        condition = pred if function == "unittest.skipIf" else "not (" + pred + ")"
        configuration = "pinned CPython 3.14.5 on Linux, unprivileged, nonterminal standard streams"
        return condition, configuration, guard.lineno
    raise ValueError(f"no release source for {name}: {reason}")
