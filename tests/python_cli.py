"""Compare the script and command-string entry points with Python's CLI."""

import argparse
import errno
import json
import os
import platform
import subprocess
import sys
import tempfile
from pathlib import Path


def check_raise_site_allocation(imre, root, run):
    fixture = Path(__file__).with_name("fixtures") / "exceptions" / "refuse_raise_site.c"
    wrapper = root / "refuse_raise_site.o"
    subprocess.run(["cc", "-c", fixture, "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_exception_with_origin,--wrap=malloc",
    ])
    source = "try:\n    raise ValueError('original')\nexcept ValueError as error:\n    print(error)\nprint('later')"
    for ordinal in (0, 1, 2):
        result = run(imre, "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_RAISE_SITE_ALLOCATION": str(ordinal),
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, b"original\nlater\n", b"refused\n" if ordinal else b"",
        ), (ordinal, result)


def call_allocation_flags(root):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "itemgetter_refuse.o"
    subprocess.run(["cc", "-c", fixtures / "itemgetter_refuse.c", "-o", wrapper], check=True)
    return "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_call_at,--wrap=imre_dyn_op_at,--wrap=imre_dyn_op,--wrap=malloc,--wrap=realloc,--wrap=imre_dyn_lit",
    ])


def check_boxed_call_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    import_path = f"import sys; sys.path.insert(0, {str(fixtures)!r}); "
    imports = import_path + "import boxed_itemgetter as fixture; "
    # The first equal-sized allocation copies the call's positional arguments.
    for ordinal, expected in ((0, b"113\n"), (2, b"caught ()\n113\n")):
        result = run(imre, "-B", "-c", imports + "fixture.constructor_allocation()", extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if ordinal else b"",
        ), result
    # Refuse the dictionary snapshot and the equally sized sort buffer separately.
    for sort, ordinal in ((False, 1), (True, 1), (True, 2)):
        source = import_path + f"import boxed_json_allocation as fixture; fixture.current({sort})"
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": "16400",
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, b"caught ()\n1025 True\n", b"refused\n",
        ), result
    for module in (False, True):
        source = import_path + f"import boxed_list_pop as fixture; fixture.copy_allocation({module})"
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": "8192",
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, b"caught ()\n1024 1024 True True True\n", b"refused\n",
        ), result
    source = import_path + "import boxed_reversed_range as fixture; fixture.search_allocation()"
    result = run(imre, "-B", "-c", source, extra={
        "IMRE_ENCODED_FLAGS": flags,
        "IMRE_TEST_CALL_SIZE": "4104",
        "IMRE_TEST_GETTER_ALLOCATION": "1",
    })
    assert (result.returncode, result.stdout, result.stderr) == (0, b"False\n", b""), result
    source = import_path + "import boxed_namedtuple_factory as fixture; fixture.layout_read_allocation()"
    result = run(imre, "-B", "-c", source, extra={
        "IMRE_ENCODED_FLAGS": flags,
    })
    assert (result.returncode, result.stdout, result.stderr) == (0, b"9\n", b""), result
    for kind in ("item", "attr"):
        source = imports + f"fixture.result_allocation({kind!r})"
        for ordinal, expected in ((0, b"113 113\n"), (1, b"caught () 0\n113 113\n")):
            result = run(imre, "-B", "-c", source, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
            })
            assert (result.returncode, result.stdout, result.stderr) == (
                0, expected, b"refused\n" if ordinal else b"",
            ), result

    for name, bound in (("append", False), ("append", True), ("insert", False), ("insert", True)):
        source = import_path + f"import boxed_list_pop as fixture; fixture.growth_allocation({name!r}, {bound})"
        for size, expected in ((0, b"129 True True\nretry 130 True\n"), (2048, b"caught ()\n128 True False\nretry 129 True\n")):
            result = run(imre, "-B", "-c", source, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "IMRE_TEST_CALL_SIZE": str(size),
                "IMRE_TEST_GETTER_ALLOCATION": "1",
            })
            assert (result.returncode, result.stdout, result.stderr) == (
                0, expected, b"refused\n" if size else b"",
            ), result

    source = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_source_exec as fixture; fixture.decode_allocation()"
    for ordinal, output in ((0, b"compiled\n7\n"), (1, b"caught ()\n7\n")):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, output, b"refused\n" if ordinal else b"",
        ), result

    source = import_path + "import boxed_list_pop as fixture; fixture.concat_allocation()"
    for size in (0, 1960):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, (b"caught ()\n" if size else b"") + b"128 117 245 True True\n", b"refused\n" if size else b"",
        ), result

    source = import_path + "import boxed_tuple_concatenation as fixture; fixture.allocation()"
    for size in (0, 16424):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, (b"caught ()\n" if size else b"") + b"1025 1028 2053 True True\n", b"refused\n" if size else b"",
        ), result

    for kind in ("list", "tuple", "self"):
        source = import_path + f"import boxed_list_pop as fixture; fixture.extend_allocation({kind!r})"
        # Refuse the source copy and the destination growth separately.
        for size in (0, 1024 if kind == "self" else 936, 2048):
            result = run(imre, "-B", "-c", source, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "IMRE_TEST_CALL_SIZE": str(size),
                "IMRE_TEST_GETTER_ALLOCATION": "1",
            })
            assert (result.returncode, result.stdout, result.stderr) == (
                0, (b"caught ()\n" if size else b"") + b"True True True\n", b"refused\n" if size else b"",
            ), result

    source = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_recursive_repr as fixture; fixture.allocation()"
    # Refuse the initial traversal stack, then its first growth on a nested list.
    for size, expected in ((0, b"40\n"), (128, b"caught ()\n40\n"), (256, b"caught ()\n40\n")):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), result

    source = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_recursive_repr as fixture; fixture.defaultdict_allocation()"
    # Save the formatted entries before invoking the live factory's repr.
    for size, expected in ((0, b"value 130 1\nretry 130 2\n"), (104, b"caught () 0\nretry 130 1\n")):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), result

    # Match the existing 113-word allocation probe with a fresh string of the same byte length.
    source = """import sys
sys.path
value = ''.join(('cache ', 'q' * 898))
marker = {}
marker[0] = 1
result = sys.intern(value)
print(result is value, sys._is_interned(value))
print(sys.intern(value) is value, sys._is_interned(value))
"""
    for ordinal, output in ((0, b"True True\nTrue True\n"), (1, b"True False\nTrue True\n")):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        assert (result.returncode, result.stdout, result.stderr) == (
            0, output, b"refused\n" if ordinal else b"",
        ), result

    # Check the small result, the base-1e9 scratch, the coefficient text, and the output copy.
    for wide, size, ordinal in ((False, 0, 1), (False, 8, 2), (True, 0, 1), (True, 568, 1), (True, 1235, 1), (True, 1235, 2)):
        source = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_recursive_repr as fixture; fixture.integer_allocation({wide})"
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        digits = b"1235 -2088 80635\n" if wide else b"2 23 23\n"
        expected = (b"caught ()\n" if size else b"value " + digits) + b"retry " + digits
        assert (result.returncode, result.stdout, result.stderr) == (0, expected, b"refused\n" if size else b""), result

    for size in (0, 128, 512, 1024):
        source = import_path + "import boxed_bytecache as fixture; fixture.long_allocation()"
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        expected = (b"caught ()\n" if size else b"value 0\n") + b"retry 0\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), result


def check_slice_stack(imre, imports, root, env):
    def output(exe, depths, stack_mib=None):
        set_stack = None
        if stack_mib is not None:
            import resource

            def set_stack():
                _, hard = resource.getrlimit(resource.RLIMIT_STACK)
                resource.setrlimit(resource.RLIMIT_STACK, (stack_mib * 1024**2, hard))

        command = imports + f"arithmetic.current({depths})"
        result = subprocess.run(
            [exe, "-IB", "-c", command], cwd=root, env=env, capture_output=True,
            preexec_fn=set_stack, timeout=60,
        )
        assert (result.returncode, result.stderr) == (0, b""), result
        return result.stdout

    def compare(depths, stack_mib=None):
        expected = output(sys.executable, depths, stack_mib)
        actual = output(imre, depths, stack_mib)
        assert actual == expected, (depths, stack_mib, actual, expected)
        return expected

    boundary = "(15500,)"
    if sys.platform != "linux" or platform.machine() != "x86_64":
        compare("")
        compare(boundary)
        return

    for stack_mib, expected in ((8, b"recursion"), (96, b"760001 ")):
        stdout = compare("", stack_mib)
        for renderer in (b"repr", b"str"):
            assert b"40000 " + renderer + b" " + expected in stdout, stdout

    exhausted = compare(boundary, 6)
    rendered = compare(boundary, 16)
    for renderer in (b"repr", b"str"):
        assert b"15500 " + renderer + b" recursion\n" in exhausted, exhausted
        assert b"15500 " + renderer + b" 294501 " in rendered, rendered

    # At 8 MiB the pinned GCC build fits 15,500 frames at 528 bytes per level.
    # Clang and the native guard use 624 bytes per level and must raise here.
    gcc_reference = sys.version_info[:3] == (3, 14, 5) and platform.python_compiler().strip() == "GCC 13.3.0"
    assert output(sys.executable, boundary, 8) == (rendered if gcc_reference else exhausted)
    assert output(imre, boundary, 8) == exhausted


def check_argument_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    cases = [
        ((), 0, b"113 113 2\n", b""),
        ((), 1, b"caught () 0\n113 113 1\n", b"refused\n"),
        ((), 2, b"113 113 2\n", b""),
        (("tail",), 0, b"113 114 2\n", b""),
        (("tail",), 1, b"caught () 0\n113 114 1\n", b"refused\n"),
    ]
    for arguments, ordinal, stdout, stderr in cases:
        result = run(imre, "-B", fixtures / "boxed_call_arguments.py", *arguments, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        assert (result.returncode, result.stdout, result.stderr) == (0, stdout, stderr), result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--compiler", type=Path, required=True)
    imre = parser.parse_args().compiler.resolve()
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("IMRE", "PYTHON"))
        }
        env["IMRE_CACHE_DIR"] = str(root / "cache")

        def run(exe, *args, extra=None, stdin=None):
            return subprocess.run(
                [exe, *args],
                cwd=root,
                env=env | (extra or {}),
                input=stdin,
                capture_output=True,
                check=False,
                timeout=60,
            )

        if sys.platform.startswith("linux"):
            flags = call_allocation_flags(root)
            check_argument_allocation(imre, run, flags)
            check_raise_site_allocation(imre, root, run)
            check_boxed_call_allocation(imre, run, flags)

        entry = root / "serve.py"
        entry.write_text("from application import make_endpoint\nprint(__name__, make_endpoint('ready')())\n")
        (root / "application.py").write_text(
            "def make_endpoint(message):\n"
            "    def endpoint():\n        return message\n"
            "    return endpoint\n"
        )
        reference = run(sys.executable, "-B", entry)
        assert (reference.returncode, reference.stdout) == (0, b"__main__ ready\n"), reference.stderr
        opted_in = run(imre, "run", entry, "--runtime-imports", "--quiet")
        assert (opted_in.returncode, opted_in.stdout) == (0, reference.stdout), opted_in.stderr
        # A cached object-runtime executable cannot satisfy a subsequent static build.
        ordinary = run(imre, "run", entry, "--quiet")
        assert ordinary.returncode != 0 and b"local functions require" in ordinary.stderr, ordinary.stderr

        (root / "reader.py").write_text(
            "for _ in range(3):\n    print(repr(input()))\n"
            "try:\n    input()\nexcept EOFError:\n    print('EOF')\n"
        )
        for exe in (sys.executable, imre):
            actual = run(exe, "-B", "-c", "import reader", stdin="one\r\n\nλ".encode())
            assert actual.returncode == 0 and not actual.stderr, actual.stderr
            assert actual.stdout == "'one\\r'\n''\n'λ'\nEOF\n".encode()

        streams = root / "streams.py"
        streams.write_bytes((Path(__file__).with_name("fixtures") / "boxed_sys_output.py").read_bytes())
        expected = run(sys.executable, "-B", streams)
        actual = run(imre, "run", streams, "--runtime-imports", "--quiet")
        assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
        assert actual.stdout == expected.stdout == "stdout π\n".encode()
        assert actual.stderr == expected.stderr == "stderr 🌍\nrestored\n".encode()

        string_io = root / "string_io.py"
        string_io.write_bytes((Path(__file__).with_name("fixtures") / "boxed_string_io.py").read_bytes())
        expected = run(sys.executable, "-B", string_io)
        actual = run(imre, "run", string_io, "--runtime-imports", "--quiet")
        assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
        assert actual.stdout == expected.stdout and actual.stderr == expected.stderr == b""

        exit_stack = Path(__file__).with_name("fixtures") / "boxed_exit_stack.py"
        expected = run(sys.executable, "-B", exit_stack)
        actual = run(imre, "run", exit_stack, "--runtime-imports", "--quiet")
        assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
        assert actual.stdout == expected.stdout and actual.stderr == expected.stderr == b""

        exit_stack_import = root / "stack.py"
        exit_stack_import.write_text("from contextlib import ExitStack\n"
                                     "s = ExitStack()\nprint(isinstance(s, ExitStack))\n"
                                     "print(s.__enter__() is s)\n")
        for mode in ([], ["--reachable-only"]):
            actual = run(imre, "run", exit_stack_import, *mode, "--quiet")
            assert (actual.returncode, actual.stdout, actual.stderr) == (0, b"True\nTrue\n", b""), actual.stderr

        # Use the native build pipeline to check the target's named version tuples.
        identity = root / "identity.py"
        identity.write_text(
            "from sys import implementation as impl, version_info as language\n"
            "print(impl.name)\n"
            "print(impl.cache_tag is None)\n"
            "version = impl.version\n"
            "print(f'{version.major}.{version.minor}.{version.micro}')\n"
            "print(version.releaselevel)\n"
            "print(version.serial)\n"
            "print(f'{language[0]}.{language.minor}.{language[2]}')\n"
        )
        target = json.loads(run(imre, "build-target").stdout)
        release = run(imre, "--version").stdout.decode().split()[1]
        native = run(imre, "run", identity, "--quiet")
        assert native.returncode == 0, native.stderr
        assert native.stdout.decode().splitlines() == [
            target["implementation"],
            "True",
            release,
            "final",
            "0",
            target["python_version"] + ".0",
        ]

        def compare(*args, extra=None, status=0, native_args=None):
            expected = run(sys.executable, *args, extra=extra)
            actual = run(imre, *(args if native_args is None else native_args), extra=extra)
            assert actual.returncode == expected.returncode == status, (
                args,
                actual.stderr,
                expected.stderr,
            )
            assert actual.stdout == expected.stdout, (
                args,
                actual.stdout,
                expected.stdout,
            )
            assert actual.stderr == expected.stderr == b"", (
                args,
                actual.stderr,
                expected.stderr,
            )

        compare("-IB", "-c", "import sys; print(__import__(sys.argv[1]).SelectorKey('f', 7, 1, 'd'))", "selectors")
        compare(
            "-IB", "-c", "import sys; sys.path; m = __import__(sys.argv[1]); "
            "sys.version = '7.42 (custom) [cc]'; print(m.python_version())", "platform",
        )

        getters = Path(__file__).with_name("fixtures") / "typed_attrgetter"
        compare("-IB", getters / "fields.py")
        compare("-B", getters / "main.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_attrgetter.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_itemgetter.py")
        inspect_fixture = Path(__file__).with_name("fixtures") / "boxed_inspect_coroutine.py"
        (root / "view_inspection.py").write_bytes(inspect_fixture.read_bytes())
        inspect_entry = root / "inspect_views.py"
        inspect_entry.write_text("import view_inspection\n")
        compare("-B", inspect_entry, native_args=("run", inspect_entry, "--runtime-imports", "--quiet"))
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_proxy_operators.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_update_wrapper.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_tracebacks.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_print_traceback.py")
        compare("-IB", "-c", "import operator as op\nprint(op.attrgetter('upper')('method')())")

        args_code = (
            "import sys\nfor arg in sys.argv:\n    print(arg)\nprint(__name__)\n"
        )
        flags_code = (
            "import sys\nprint(sys.flags.isolated)\nprint(sys.flags.ignore_environment)\n"
            "print(sys.flags.safe_path)\nprint(sys.flags.no_user_site)\n"
            "print(sys.flags.dont_write_bytecode)\nprint(sys.dont_write_bytecode)\n"
        )
        compare("-I", "-B", "-c", args_code, "--help", "--", "two words", "π")
        compare("-IBc" + flags_code)
        compare("-c", flags_code, extra={"PYTHONDONTWRITEBYTECODE": "1"})
        compare(
            "-c",
            flags_code,
            extra={
                "PYTHONDONTWRITEBYTECODE": "0",
                "PYTHONNOUSERSITE": "1",
                "PYTHONSAFEPATH": "0",
            },
        )
        compare(
            "-I",
            "-c",
            flags_code,
            extra={"PYTHONDONTWRITEBYTECODE": "1", "IMREFLAGS": "--bogus"},
        )
        compare("-B", "-c", "import sys\nsys.exit(37)", status=37)
        compare("-IB", "-c", "raise SystemExit")
        compare("-IB", "-c", "from sys import exit\nexit(None)")
        for source in ('import sys\nsys.exit("7")', 'raise SystemExit("7")'):
            actual = run(imre, "-IB", "-c", source)
            assert (actual.returncode, actual.stdout, actual.stderr) == (1, b"", b"7\n")
        cleanup = Path(__file__).resolve().parent / "fixtures/exceptions/boxed_finally.py"
        compare(cleanup, extra={"PYTHONWARNINGS": "ignore::SyntaxWarning"})

        # Bundled children must respect the selected package's __path__.
        shadow = root / "shadow"
        (shadow / "http").mkdir(parents=True)
        (shadow / "http" / "__init__.py").write_text("marker = 17\n")
        shadow_entry = shadow / "entry.py"
        shadow_entry.write_text(
            "import sys, http\nsys.path\nassert http.marker == 17\n"
            "try:\n    import http.client\nexcept ModuleNotFoundError:\n    print('local package')\n"
        )
        compare("-B", shadow_entry)

        # Accept genuine U+FFFD, raw text, and literal backslashes on the boxed path.
        compare(
            "-c",
            r"""import sys
print(r"\uDCFF", "\\uDCFF", "�", "\ufffd", "\U0001f642", br"\uDCFF", b"\uDCFF")
print(rf"\uDCFF{r'\uDCFF'}")
sys.path.append("unused")""",
            extra={"PYTHONWARNINGS": "ignore::SyntaxWarning"},
        )
        surrogate = b"surrogate string escapes are unsupported"
        for expression in (
            r'"surrogates:\uDCFF"',
            r'f"{3}\U0000d800"',
            r'''rf"\uDCFF{'\ud800'}"''',
            r'f"{3:\udcff}"',
        ):
            result = run(imre, "-c", f"import sys; sys.exit({expression})")
            assert (result.returncode, result.stdout) == (1, b""), result
            assert surrogate in result.stderr, result.stderr
        (root / "surrogate_module.py").write_text(r'value = "\udfff"')
        imported = run(
            imre,
            "-c",
            "import sys, surrogate_module; sys.path.append('unused'); print(surrogate_module.value)",
        )
        assert (imported.returncode, imported.stdout) == (1, b""), imported
        assert b"surrogate_module.py:1:" in imported.stderr, imported.stderr
        assert surrogate in imported.stderr, imported.stderr
        assert run(imre, "-c").returncode == 2
        assert run(imre, "-I", "missing.py").returncode == 2

        imports = root / "import_members"
        imports.mkdir()
        (imports / "__init__.py").write_text("")
        (imports / "child.py").write_text("import missing_child_dependency\n")
        (imports / "found.py").write_text("value = 37\n")
        compare(
            "-B", "-c",
            """from sys import path
try:
    from import_members import absent
except ModuleNotFoundError:
    print("missing module")
except ImportError as error:
    print(error.args, error.name, error.path, error.name_from)
try:
    from sys import missing_export
except ImportError as error:
    print(error.msg, error.name, error.path, error.name_from)
try:
    from import_members import child
except ModuleNotFoundError as error:
    print(error.msg, error.name)
from import_members import found
print(found.value)
""",
        )

        diagnostic = run(imre, "-c", "print(__file__)")
        assert diagnostic.returncode != 0 and b"<string>" in diagnostic.stderr, (
            diagnostic.stderr
        )

        package = root / "pkg"
        package.mkdir()
        # Running a script directly does not initialize its containing package.
        (package / "__init__.py").write_text(
            "raise RuntimeError('package initialized')\n"
        )
        (package / "app.py").write_text(args_code + "print(__file__)\n")
        compare("pkg/app.py", "-B", "--help", "--", "last")
        compare("-I", "-B", "pkg/app.py", "one")
        compare("--", "pkg/app.py", "two")

        module = "from neighbor import answer\nprint(answer())\n"
        (root / "neighbor.py").write_text("def answer() -> int:\n    return 11\n")
        (package / "neighbor.py").write_text("def answer() -> int:\n    return 22\n")
        (package / "app.py").write_text(module)
        compare("-B", "-c", module)
        compare("-B", "pkg/app.py")
        if os.name == "posix":
            (package / "path0.py").write_text(
                "import sys\nprint(sys.path[0])\nprint(__file__)\n" + module
            )
            (root / "linked-script.py").symlink_to("pkg/path0.py")
            compare("-B", "linked-script.py")

            byte_dir = root / os.fsdecode(b"dir-\xff")
            byte_dir.mkdir()
            (byte_dir / "main.py").write_text("import sys\nprint(sys.path[0])\n")
            (root / "ascii-alias").symlink_to(byte_dir.name, target_is_directory=True)
            result = run(imre, "-B", "ascii-alias/main.py")
            assert (result.returncode, result.stdout, result.stderr) == (
                0, os.fsencode(root.resolve() / "ascii-alias") + b"\n", b"",
            ), result
        assert (
            run(imre, "-B", "pkg/app.py", extra={"PYTHONSAFEPATH": "1"}).returncode
            != 0
        )
        for source in (["-c", module], ["pkg/app.py"]):
            result = run(imre, "-I", "-B", *source)
            assert result.returncode != 0 and b"neighbor" in result.stderr, (
                result.stderr
            )

        installed = root / "installed"
        installed.mkdir()
        (installed / "only_on_path.py").write_text(
            "def answer() -> int:\n    return 33\n"
        )
        code = module.replace("neighbor", "only_on_path")
        extra = {"PYTHONPATH": str(installed)}
        compare("-B", "-c", code, extra=extra)
        assert run(imre, "-I", "-c", code, extra=extra).returncode != 0
        assert not list(root.rglob("*.pyc"))

        executable = run(
            imre,
            "-I",
            "-c",
            "import sys\nprint(sys.executable)",
            extra={"IMRE_PYTHON": "/wrong"},
        )
        assert executable.stdout.decode().strip() == str(imre), executable.stderr

        # An isolated command can add a package search root itself. Exercise its
        # normal Python code, including live dictionary views and descriptor errors.
        helpers = root / "helpers"
        helpers.mkdir()
        (helpers / "report.py").write_text(
            """\
class Item:
    @property
    def name(self):
        raise ValueError("lookup failed")

def report(text):
    fields = {"name": "Imre"}
    view = fields.items()
    fields.update(count=len(text.splitlines(keepends=True)))
    print("{{{name}}}: {count}".format(**fields))
    print("%(name)s/%(count)d" % fields)
    for key, value in view:
        print(key, value)
    print(text.rsplit(None, 1)[0])
    try:
        text.split("")
    except ValueError:
        print("empty separator")
    try:
        getattr(Item(), "name", "default")
    except ValueError as err:
        print(err)
    try:
        fields[(["mutable"],)] = 1
    except TypeError:
        print("unhashable key")
"""
        )
        compare(
            "-I",
            "-B",
            "-c",
            f"import sys\nprint({str(helpers)!r} in sys.path)\n"
            f"sys.path.insert(0, {str(helpers)!r})\nprint(sys.path[0])\n"
            "from report import report\nreport('a\\x1c  b\\r\\nc')\n",
        )

        decimal_import = f"import sys\nsys.path.insert(0, {str(helpers)!r})\n"
        (helpers / "decimal_values.py").write_text(
            "from decimal import Decimal\n"
            "x = Decimal('1.2300')\n"
            "print(x, Decimal(value=x) is x, Decimal(True), len(str(Decimal(10 ** 5000))))\n"
            "class Text(str):\n"
            "    def strip(self): raise AssertionError('unexpected strip')\n"
            "print(Decimal(Text('2.500')), Decimal(value=Text('-0.0')))\n"
        )
        compare("-IB", "-c", decimal_import + "import decimal_values")
        (helpers / "decimal_subclass.py").write_text(
            "from decimal import Decimal\n"
            "class Amount(Decimal): pass\n"
            "x = Amount('1.2300')\n"
            "for result in (Amount(12), Amount(False), Amount(Decimal('1.2300')), Decimal(x), Amount(x)):\n"
            "    print(type(result).__name__, result, result is x)\n"
        )
        compare("-IB", "-c", decimal_import + "import decimal_subclass")
        (helpers / "decimal_comparison.py").write_text(
            "from decimal import Decimal\nprint(Decimal('1.00') == Decimal(1))\n"
        )
        unsupported = run(imre, "-IB", "-c", decimal_import + "import decimal_comparison")
        assert unsupported.returncode and b"native Decimal numeric comparisons are unsupported" in unsupported.stderr

        (helpers / "decimal.py").write_text("class Decimal:\n    def __init__(self, value): self.value = value\n")
        compare("-IB", "-c", decimal_import + "from decimal import Decimal\nprint(Decimal(False).value is False)")

        (helpers / "uuid").mkdir()
        uuid_import = f"import sys\nsys.path.insert(0, {str(helpers)!r})\nimport uuid\n"
        compare("-IB", "-c", uuid_import + "print(hasattr(uuid, '__path__'), str(uuid.UUID(int=0)))")
        (helpers / "uuid.py").write_text("marker = 'explicit search path'\n")
        compare("-IB", "-c", uuid_import + "print(uuid.marker)")

        (root / "legacy.py").write_text(args_code)
        legacy = run(imre, "run", "legacy.py", "--quiet", "--", "--opt-level", "2")
        assert (
            legacy.returncode == 0
            and legacy.stdout == b"legacy.py\n--opt-level\n2\n__main__\n"
        ), legacy.stderr
        # A native cache, when available, keys command text and its import policy.
        trace = {"IMRE_CACHE_TRACE": "1"}
        command = ("-I", "-B", "-c", "print(101)")
        first = run(imre, *command, extra=trace)
        assert first.stdout == b"101\n", first.stderr
        if b"imre cache store" in first.stderr:
            again = run(imre, *command, extra=trace)
            assert again.stdout == first.stdout and b"imre cache hit" in again.stderr
            changed = run(imre, *command[:-1], "print(102)", extra=trace)
            assert changed.stdout == b"102\n" and b"imre cache miss" in changed.stderr
        # The literal path is made available even under -I. Both loop iterators must
        # stay live when the native runtime collects temporaries inside the inner loop.
        plugin = installed / "arithmetic.py"
        source = (
            "def total():\n"
            "    n = 0\n"
            "    for i in range(1400):\n"
            "        for j in [i, i + 1]:\n"
            "            n += j\n"
            "    return n\n"
            "def fail():\n"
            "    raise ValueError('from plugin')\n"
        )
        plugin.write_text(source)
        roots = [str(installed), str(installed)]
        imports = f"import sys\nsys.path = {roots!r} + sys.path\nimport arithmetic\n"
        compare("-IB", "-c", imports + f"print(arithmetic.total())\nprint(sys.path[:2] == {roots!r})")
        plugin.write_text(source.replace("n = 0", "n = 7"))
        compare("-IB", "-c", imports + "print(arithmetic.total())")
        error = run(imre, "-IB", "-c", imports + "arithmetic.fail()")
        assert error.returncode == 1 and error.stdout == b"", error.stderr
        assert b'File "<string>", line 4' in error.stderr, error.stderr
        assert f'File "{plugin}", line 8'.encode() in error.stderr, error.stderr
        assert error.stderr.rstrip().endswith(b"ValueError: from plugin"), error.stderr
        resource_package = installed / "pkgutil_resource"
        resource_package.mkdir()
        (resource_package / "__init__.py").write_text("print('resource package initialized')\n")
        (resource_package / "payload.bin").write_bytes(b"\x00\xff\r\n")
        fixtures = Path(__file__).with_name("fixtures")
        for fixture in (
            "boxed_ast.py",
            "boxed_ast_literal_eval.py",
            "boxed_code_objects.py",
            "boxed_recursive_repr.py",
            "boxed_namedtuple_repr.py",
            "boxed_namedtuple_factory.py",
            "boxed_slice_deep_repr.py",
            "boxed_slice_repr_cycles.py",
            "boxed_source_exec.py",
            "boxed_sys_intern.py",
            "boxed_bytes_add.py",
            "boxed_bytes_io.py",
            "boxed_text_io_wrapper.py",
            "boxed_platform_version.py",
            "boxed_int_to_bytes.py",
            "boxed_complex.py",
            "boxed_float.py",
            "boxed_round.py",
            "boxed_math_rounding.py",
            "boxed_float_arithmetic.py",
            "boxed_bytes_fields.py",
            "boxed_class_union.py",
            "boxed_bytes_octal.py",
            "boxed_contextvars.py",
            "boxed_temporary_file.py",
            "boxed_random.py",
            "boxed_secrets.py",
            "boxed_pkgutil.py",
            "boxed_lru_cache_equality.py",
            "boxed_lru_insert_eviction.py",
            "boxed_lru_insert_same.py",
            "boxed_typing_optional.py",
            "boxed_typing_literal.py",
            "boxed_typing_protocol.py",
            "boxed_formatter.py",
            "boxed_re_ascii.py",
            "boxed_re_verbose.py",
            "boxed_re_split.py",
            "boxed_re_findall_empty.py",
            "boxed_datetime.py",
            "boxed_dataclasses.py",
            "boxed_email_dates.py",
            "boxed_time.py",
            "boxed_codecs.py",
            "boxed_selector_wide_fds.py",
            "boxed_pipe_flags.py",
            "boxed_os_file_descriptors.py",
            "boxed_select.py",
            "boxed_urlsplit.py",
            "boxed_unicodedata.py",
            "boxed_descriptors.py",
            "boxed_exception_repr.py",
            "boxed_oserror.py",
            "boxed_type_identity.py",
            "boxed_singleton_types.py",
            "boxed_singleton_subclass.py",
            "boxed_function_closure_cells.py",
            "boxed_function_closure_edges.py",
            "boxed_functiontype.py",
            "boxed_any_generator.py",
            "boxed_global.py",
            "boxed_logging.py",
            "boxed_class_attribute.py",
            "boxed_request_strings.py",
            "boxed_str_maketrans.py",
            "boxed_map_capwords.py",
            "boxed_module_globals.py",
            "boxed_dir.py",
            "boxed_frozenset.py",
            "boxed_attribute_deletion.py",
            "boxed_recursion.py",
            "boxed_function_defaults.py",
            "boxed_set_union.py",
            "boxed_weakref.py",
            "boxed_public_weakref.py",
            "boxed_weakref_subclass.py",
            "boxed_proxy_truth.py",
            "boxed_setattr.py",
            "boxed_super.py",
            "boxed_property_setter.py",
            "boxed_staticmethod.py",
            "boxed_item_assignment.py",
            "boxed_bytecache.py",
            "boxed_fnmatch.py",
            "boxed_posixpath.py",
            "boxed_stat.py",
            "boxed_itertools_count.py",
            "boxed_enum_descriptors.py",
            "boxed_enum_auto.py",
            "boxed_print_streams.py",
            "boxed_redirect_streams.py",
            "boxed_itertools_repeat.py",
            "boxed_itertools_chain.py",
            "boxed_itertools_groupby.py",
            "boxed_module_type.py",
            "boxed_thread_local.py",
            "boxed_threading_lock.py",
            "boxed_dict_subclasses.py",
            "boxed_mapping_abc.py",
            "boxed_dict_pop.py",
            "boxed_recursive_equality.py",
            "boxed_selector_modify_data.py",
            "boxed_structural_hooks.py",
            "boxed_list_pop.py",
            "boxed_sequence_repeat.py",
            "boxed_sequence_order.py",
            "boxed_json_containers.py",
            "boxed_deque.py",
            "boxed_deque_deep_equal.py",
            "boxed_deque_cycle_callbacks.py",
            "boxed_deque_mixed_equal.py",
            "boxed_deque_nested_mutation.py",
            "boxed_zip.py",
            "boxed_map.py",
            "boxed_python_iterators.py",
            "boxed_float_sum.py",
            "boxed_complex_sum.py",
            "boxed_enumerate_methods.py",
            "boxed_enumerate_metadata.py",
            "boxed_sorted.py",
            "boxed_max.py",
            "boxed_min.py",
            "boxed_generator_delegation.py",
            "boxed_slice.py",
            "boxed_binascii.py",
            "boxed_zlib.py",
            "boxed_standard_base64.py",
            "boxed_bytes_separators.py",
            "boxed_struct.py",
            "boxed_reversed_list.py",
            "boxed_reversed_range.py",
            "boxed_heapq.py",
            "boxed_difflib.py",
            "boxed_string_new.py",
            "boxed_string_repeat.py",
            "boxed_string_expandtabs.py",
        ):
            if fixture == "boxed_stat.py" and sys.platform not in ("linux", "darwin"):
                continue
            plugin.write_bytes((fixtures / fixture).read_bytes())
            if fixture == "boxed_slice_deep_repr.py":
                check_slice_stack(imre, imports, root, env)
            else:
                compare("-IB", "-c", imports + "arithmetic.current()")
        if os.name == "posix":
            plugin.write_bytes((fixtures / "boxed_mimetypes.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            if fixture == "boxed_recursive_repr.py":
                for limit in (640, 4300, 0):
                    compare(
                        "-B", "-c", imports + f"arithmetic.integer_digit_limit({limit})",
                        extra={"PYTHONINTMAXSTRDIGITS": str(limit)},
                    )
        if sys.platform == "linux":
            shim = root / "secrets_entropy.so"
            subprocess.run(["cc", "-shared", "-fPIC", fixtures / "secrets_entropy.c", "-ldl", "-o", shim], check=True)
            probe = root / "entropy.py"
            probe.write_bytes((fixtures / "boxed_secrets.py").read_bytes() + b"\nentropy()\n")
            injected = {"LD_PRELOAD": str(shim), "PYTHONHASHSEED": "1", "IMRE_SECRETS_TEST_SOURCE": "fail"}
            oracle = run(sys.executable, "-B", probe, extra=injected)
            assert oracle.returncode == 0 and oracle.stdout.startswith(b"OSError (5,") and not oracle.stderr, oracle
            for opt in ("0", "3"):
                binary = root / f"entropy-O{opt}"
                build = run(imre, "build", probe, "--runtime-imports", "--opt-level", opt, "-o", binary)
                assert build.returncode == 0, build.stderr
                fault = run(binary, extra=injected)
                assert (fault.returncode, fault.stdout, fault.stderr) == (0, oracle.stdout, b""), fault
                retry = run(binary, extra=injected | {"IMRE_SECRETS_TEST_SOURCE": "reject"})
                assert (retry.returncode, retry.stdout, retry.stderr) == (0, b"c\n", b"draw\ndraw\n"), retry
        plugin.write_bytes((fixtures / "boxed_typing_generics.py").read_bytes())
        compare("-IB", "-c", imports)
        plugin.write_bytes((fixtures / "boxed_class_docstrings.py").read_bytes())
        for flags in ("-IB", "-IBOO"):
            compare(flags, "-c", imports + "arithmetic.current()")
        for first_import in ("import collections", "import _collections_abc"):
            plugin.write_text(f"""{first_import}
import collections
print(collections.abc.__name__, collections.__package__, bool(collections.__path__))
import collections.abc as dotted
from collections import abc
import _collections_abc
print(abc is dotted, abc is _collections_abc, repr(abc.__package__))
try:
    import collections.unknown_child
except ImportError:
    print('missing child')
""")
            compare("-IB", "-c", imports)
        http = fixtures.parents[1] / "crates/imre-compiler/stdlib/http/__init__.py"
        plugin.write_bytes(http.read_bytes())
        compare("-IB", "-c", imports + (fixtures / "boxed_http_enums.py").read_text())
        errno_names = [name for name, value in vars(errno).items() if type(value) is int]
        compare(
            "-IB",
            "-c",
            "from sys import path\nimport errno\nfrom errno import errorcode\n"
            f"print([getattr(errno, name) for name in {errno_names!r}])\n"
            "print(list(errorcode.items()))\n"
            "errorcode[errno.EADDRINUSE] = 'changed'\n"
            "print(errno.errorcode is errorcode, errno.errorcode[errno.EADDRINUSE])\n",
        )
        (root / "chunks.bin").write_bytes(b"0a\r\n\xffx!\n")
        plugin.write_bytes((Path(__file__).with_name("fixtures") / "boxed_io.py").read_bytes())
        compare("-IB", "-c", imports + "arithmetic.exercise('chunks.bin')")
        compare(
            "-IB", "-c",
            "import sys, hashlib\nsys.path\n"
            "def factory(data=b''):\n    return hashlib.sha1(data)\n"
            "class Flag:\n    def __bool__(self):\n        raise ValueError('hash flag')\n"
            "h = factory(b'\\x00a')\nalias = h\nalias.update(b'\\xff')\n"
            "copy = h.copy()\ncopy.update(b'next')\n"
            "print(h.name, h.block_size, h.digest_size, list(h.digest()), h.hexdigest())\n"
            "print(list(h.digest()), list(copy.digest()), factory().hexdigest())\n"
            "print(hashlib.sha1(usedforsecurity=False).hexdigest() == factory().hexdigest())\n"
            "try:\n    hashlib.sha1(b'a', data=b'b', usedforsecurity=Flag())\n"
            "except TypeError:\n    print('duplicate data')\n"
            "try:\n    hashlib.sha1('bad', usedforsecurity=Flag())\n"
            "except ValueError as exc:\n    print(str(exc))\n"
            "try:\n    h.update('text')\nexcept TypeError as exc:\n    print(str(exc))\n",
        )
        plugin.write_bytes((Path(__file__).with_name("fixtures") / "boxed_hmac.py").read_bytes())
        compare("-IB", "-c", imports + "arithmetic.check()")
        plugin.write_bytes((Path(__file__).with_name("fixtures") / "boxed_text_output.py").read_bytes())
        compare("-IB", "-c", imports + "arithmetic.exercise('port.txt')")
        if Path("/dev/full").exists():
            compare("-IB", "-c", imports + "arithmetic.full_device()")
        plugin.write_bytes((Path(__file__).with_name("fixtures") / "boxed_urlsafe_base64.py").read_bytes())
        compare("-IB", "-c", imports + "arithmetic.check()")
        checked = run(
            imre,
            "-IB",
            "-c",
            "import sysconfig\n"
            "def identity(x: int) -> int:\n    return x\n"
            "def wrong() -> bytes:\n    return 42\n"
            "try:\n    identity('wrong')\nexcept TypeError:\n    print('argument rejected')\n"
            "try:\n    wrong()\nexcept TypeError:\n    print('return rejected')\n",
        )
        assert checked.returncode == 0 and checked.stderr == b"", checked.stderr
        assert checked.stdout == b"argument rejected\nreturn rejected\n", checked.stdout
        compare(
            "-IB",
            "-c",
            """\
import json, struct, sysconfig
high = 1 << 63
wide = 0x1_0000_0000_0000_0000_0000_0011
print(high, high + high, -high - 1, wide > high, int(str(wide)) == wide)
print((-wide) >> 99, wide >> (wide + 1), (-wide) >> wide, 0 << wide)
print(2 ** 80, (wide * 17 - 1) // -3, (-wide) % 11, ~wide)
print("%d" % wide, f"{-wide:X}", json.dumps({wide: int("0x_f", 0) + wide}))
print(struct.unpack(">Q", bytes([255, 255, 255, 255, 255, 255, 255, 255])))
try:
    high << -1
except ValueError as err:
    print(err)
""",
        )
        compare(
            "-IB",
            "-c",
            """\
import json, sysconfig

def fallback(value):
    return ["fallback", value.label]

class Extra:
    label = "π"

data = json.loads(b'{"values": [39614081257132168796771975168, 3, -2]}')
values = data["values"]
options = {"default": fallback, "separators": (",", ":")}
options.setdefault("default", None)
options.setdefault("ensure_ascii", True)
options.setdefault("sort_keys", True)
print(json.dumps({"total": values[0] + values[1] + values[2], "count": len(values)}, **options))
try:
    json.loads(b"{}")["values"]
except KeyError as err:
    print(str(err))
print(json.dumps({"ok": True}, **options))
print(json.dumps({"z": Extra(), "a": json.loads("1.25")}, **options))
print(json.dumps({2: False, json.loads("0.5"): True}, sort_keys=True))
from enum import IntEnum
class Code(IntEnum):
    ok = 4
class Label(str):
    pass
class DecimalKey(float):
    pass
print(json.dumps({Label("status"): [Code.ok, DecimalKey(1.25)]}, **options))
print(json.dumps({Code.ok: Label("ok"), DecimalKey(2.5): Code.ok}, **options))
nan = json.loads("NaN")
print(nan == nan, nan <= 1, [nan] == [nan])
print(json.dumps([1.25], allow_nan=False), json.dumps({nan: nan}, allow_nan=True))
for obj in ([nan], {float("-inf"): 0}):
    try:
        json.dumps(obj, allow_nan=False)
    except ValueError as err:
        print(str(err))
defaults = {"present": None}
print(defaults.setdefault("missing"), defaults.setdefault("present", 1), len(defaults))
cache = (Ellipsis, Ellipsis)
print(cache[False] is ..., cache[True] is Ellipsis, repr(cache[False]))
class BrokenFlag:
    def __bool__(self):
        raise ValueError("invalid JSON option")
try:
    json.dumps({"ok": True}, sort_keys=BrokenFlag())
except ValueError as err:
    print(str(err))
try:
    json.loads(Ellipsis)
except TypeError as err:
    print(str(err))
data = json.loads('{"k": 1, "other": [true], "k": 2}')
data["other"].append(False)
print(json.dumps(data))
try:
    json.loads(b'{"values": [1,]}')
except ValueError as err:
    print(isinstance(err, json.JSONDecodeError), str(err))
""",
        )
    print("Python script and command-line semantics match CPython")


if __name__ == "__main__":
    main()
