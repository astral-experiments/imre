"""Compare the script and command-string entry points with Python's CLI."""

import argparse
import errno
import json
import os
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

        def run(exe, *args, extra=None):
            return subprocess.run(
                [exe, *args],
                cwd=root,
                env=env | (extra or {}),
                capture_output=True,
                check=False,
                timeout=60,
            )

        if sys.platform.startswith("linux"):
            check_raise_site_allocation(imre, root, run)

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

        getters = Path(__file__).with_name("fixtures") / "typed_attrgetter"
        compare("-IB", getters / "fields.py")
        compare("-B", getters / "main.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_attrgetter.py")
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
        imports = f"import sys\nsys.path = [{str(installed)!r}] + sys.path\nimport arithmetic\n"
        compare("-IB", "-c", imports + "print(arithmetic.total())")
        plugin.write_text(source.replace("n = 0", "n = 7"))
        compare("-IB", "-c", imports + "print(arithmetic.total())")
        error = run(imre, "-IB", "-c", imports + "arithmetic.fail()")
        assert error.returncode == 1 and error.stdout == b"", error.stderr
        assert b'File "<string>", line 4' in error.stderr, error.stderr
        assert f'File "{plugin}", line 8'.encode() in error.stderr, error.stderr
        assert error.stderr.rstrip().endswith(b"ValueError: from plugin"), error.stderr
        fixtures = Path(__file__).with_name("fixtures")
        for fixture in (
            "boxed_ast.py",
            "boxed_code_objects.py",
            "boxed_recursive_repr.py",
            "boxed_source_exec.py",
            "boxed_bytes_add.py",
            "boxed_bytes_io.py",
            "boxed_int_to_bytes.py",
            "boxed_complex.py",
            "boxed_float.py",
            "boxed_float_arithmetic.py",
            "boxed_bytes_octal.py",
            "boxed_contextvars.py",
            "boxed_temporary_file.py",
            "boxed_typing_protocol.py",
            "boxed_formatter.py",
            "boxed_re_ascii.py",
            "boxed_re_verbose.py",
            "boxed_re_split.py",
            "boxed_datetime.py",
            "boxed_email_dates.py",
            "boxed_time.py",
            "boxed_codecs.py",
            "boxed_selector_wide_fds.py",
            "boxed_urlsplit.py",
            "boxed_descriptors.py",
            "boxed_exception_repr.py",
            "boxed_type_identity.py",
            "boxed_singleton_types.py",
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
            "boxed_frozenset.py",
            "boxed_attribute_deletion.py",
            "boxed_recursion.py",
            "boxed_function_defaults.py",
            "boxed_proxy_truth.py",
            "boxed_setattr.py",
            "boxed_super.py",
            "boxed_property_setter.py",
            "boxed_staticmethod.py",
            "boxed_item_assignment.py",
            "boxed_bytecache.py",
            "boxed_fnmatch.py",
            "boxed_posixpath.py",
            "boxed_itertools_count.py",
            "boxed_enum_descriptors.py",
            "boxed_print_streams.py",
            "boxed_redirect_streams.py",
            "boxed_itertools_repeat.py",
            "boxed_thread_local.py",
            "boxed_threading_lock.py",
            "boxed_dict_subclasses.py",
            "boxed_dict_pop.py",
            "boxed_recursive_equality.py",
            "boxed_list_pop.py",
            "boxed_zip.py",
            "boxed_map.py",
            "boxed_python_iterators.py",
            "boxed_enumerate_methods.py",
            "boxed_enumerate_metadata.py",
            "boxed_sorted.py",
            "boxed_generator_delegation.py",
            "boxed_slice.py",
            "boxed_binascii.py",
            "boxed_standard_base64.py",
            "boxed_bytes_separators.py",
            "boxed_struct.py",
            "boxed_reversed_list.py",
            "boxed_string_new.py",
            "boxed_string_repeat.py",
            "boxed_string_expandtabs.py",
        ):
            plugin.write_bytes((fixtures / fixture).read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
        if os.name == "posix":
            plugin.write_bytes((fixtures / "boxed_mimetypes.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
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
nan = json.loads("NaN")
print(nan == nan, nan <= 1, [nan] == [nan])
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
