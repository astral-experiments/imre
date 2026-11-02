"""Compare the script and command-string entry points with Python's CLI."""

import argparse
import errno
import importlib.util
import json
import os
import platform
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path


def check_command_stdin(imre, run):
    source = (
        "import os, sys\n"
        "from io import BytesIO, StringIO\n"
        "s = sys.stdin\n"
        "assert s is sys.__stdin__ and s.fileno() == 0 and not s.buffer.raw.closefd\n"
        "print(s.name, repr(input()), repr(s.readline()), repr(s.read(1)), repr(s.read()), repr(s.read()))\n"
        "s.close()\n"
        "print(os.read(0, 1))\n"
        "sys.stdin = StringIO('redirected\\n')\n"
        "print(input())\n"
        "sys.stdin = BytesIO(b'binary\\n')\n"
        "modules = sys.modules\n"
        "original_sys = modules['sys']\n"
        "modules['sys'] = None\n"
        "result = input()\n"
        "modules['sys'] = original_sys\n"
        "print(result)\n"
    )
    # Hold the write end open until each read returns. The alarm also bounds the
    # compiled child if a regression makes it wait for another write or EOF.
    live_pipe = (
        "import os, signal, sys\n"
        "read_fd, write_fd = os.pipe()\n"
        "os.dup2(read_fd, 0)\n"
        "os.close(read_fd)\n"
        "signal.alarm(5)\n"
        "os.write(write_fd, b'line\\n')\n"
        "assert input() == 'line'\n"
        "buffer = sys.stdin.buffer\n"
        "os.write(write_fd, b'ab')\n"
        "assert buffer.read1(0) == b'' and buffer.read1(1) == b'a'\n"
        "assert os.read(0, 1) == b'b'\n"
        "os.write(write_fd, b'xyz')\n"
        "assert buffer.read(1) == b'x' and buffer.read1(-1) == b'yz'\n"
        "os.close(write_fd)\n"
        "assert buffer.read1() == b''\n"
        "signal.alarm(0)\n"
    )
    last_input_reference = (
        "import sys, weakref\n"
        "wrapper = weakref.ref(sys.stdin)\n"
        "buffer = sys.stdin.buffer\n"
        "sys.stdin = None\n"
        "assert wrapper() is sys.__stdin__ and not buffer.closed\n"
        "vars(sys)['__stdin__'] = None\n"
        "assert wrapper() is None and buffer.closed\n"
    )
    cases = [
        (last_input_reference, b""),
        ("import sys; print(repr(sys.stdin.read()))", b""),
        (source, "one\r\ntwo\nλtail".encode()),
    ]
    if os.name == "posix":
        cases.append((live_pipe, b""))
    for command, data in cases:
        reference = run(sys.executable, "-B", "-c", command, stdin=data)
        actual = run(imre, "-B", "-c", command, stdin=data)
        assert reference.returncode == actual.returncode == 0, (reference.stderr, actual.stderr)
        assert actual.stdout == reference.stdout and actual.stderr == reference.stderr == b""


def check_sysconfig_build(imre, root, run):
    by_name = (
        "import _imp, importlib, sys\n"
        "mod = importlib.import_module(sys.argv[1])\n"
        "assert mod.get_config_var('EXT_SUFFIX') in _imp.extension_suffixes()\n"
        "assert mod.get_config_vars() is mod.get_config_vars()\n"
    )
    flags = "\x1f".join(["--runtime-imports", "--no-cache", "--opt-level", "0"])
    for executable in (sys.executable, imre):
        result = run(executable, "-B", "-c", by_name, "sysconfig",
                     extra={"IMRE_ENCODED_FLAGS": flags}, timeout=180)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"", b""), result
    fixture = Path(__file__).with_name("fixtures") / "boxed_sysconfig_build.py"
    project = root / "build-tree"
    modules = project / "Modules"
    modules.mkdir(parents=True)
    alias = root / "build-link"
    alias.symlink_to(project, target_is_directory=True)
    empty = root / "empty-build-tree"
    (empty / "Modules" / "Setup").mkdir(parents=True)
    for home, extra in ((project, {}), (empty, {"_PYTHON_PROJECT_BASE": alias.name})):
        (modules / "Setup").unlink(missing_ok=True)
        (modules / "Setup.local").touch()
        result = run(imre, "-B", fixture, project, home, empty, extra=extra)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"build markers passed\n", b""), result


def check_io_compaction_allocation(imre, root, run):
    fixture = Path(__file__).with_name("fixtures") / "refuse_io_compaction.c"
    wrapper = root / "refuse_io_compaction.o"
    subprocess.run(["cc", "-c", fixture, "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=malloc,--wrap=realloc,--wrap=calloc,--wrap=free",
    ])
    payload = root / "short-read.bin"
    payload.write_bytes(b"0123456789abcdefGHIJKLMNOPQRSTUV")
    for boxed in (False, True):
        prefix = 'import sys; sys.path.append("unused")\n' if boxed else ""
        source = prefix + (
            f"stream = open({str(payload)!r}, 'rb')\n"
            "print(stream.read(4096) == b'0123456789abcdefGHIJKLMNOPQRSTUV')\n"
            "print(stream.read() == b'')\nstream.close()\n"
        )
        for refuse in (0, 1):
            result = run(imre, "-c", source, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "IMRE_TEST_IO_COMPACTION_ALLOCATION": str(refuse),
            })
            assert (result.returncode, result.stdout, result.stderr) == (
                0, b"True\nTrue\n", b"refused\n" if refuse else b"",
            ), (boxed, refuse, result)
        for receive_flags in (0, 2):
            source = prefix + f"""import socket
listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen(1)
sender = socket.socket()
sender.connect(listener.getsockname())
receiver, _ = listener.accept()
sender.sendall(b"0123456789abcdefGHIJKLMNOPQRSTUV")
sender.shutdown(socket.SHUT_WR)
print(receiver.recv(4096, {receive_flags}) == b"0123456789abcdefGHIJKLMNOPQRSTUV")
print(receiver.recv(1) == {b"0" if receive_flags else b""!r})
receiver.close()
sender.close()
listener.close()
"""
            for refuse in (0, 1):
                result = run(imre, "-c", source, extra={
                    "IMRE_ENCODED_FLAGS": flags,
                    "IMRE_TEST_IO_COMPACTION_ALLOCATION": str(refuse),
                })
                assert (result.returncode, result.stdout, result.stderr) == (
                    0, b"True\nTrue\n", b"refused\n" if refuse else b"",
                ), (boxed, receive_flags, refuse, result)


def check_context_protocol_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "context_protocol_refuse.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_context_protocol.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_op_at,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    for opcode in ("lookup_enter", "lookup_exit"):
        for target in [0, 1]:
            result = run(imre, "-B", fixtures / "boxed_context_protocol_allocation.py", opcode, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "CONTEXT_REFUSE_NTH": str(target),
                "CONTEXT_REFUSE_OPCODE": opcode,
            })
            assert result.returncode == 0, result
            if target:
                assert result.stdout == b"kind MemoryError\n", result
                assert b"context_allocation 1 " in result.stderr and b" 1\n" in result.stderr, result
            else:
                assert result.stdout.startswith(b"kind TypeError\ndetail "), result
    for mode in ("wb", "w"):
        result = run(imre, "-B", fixtures / "boxed_context_protocol_allocation.py", mode, root / "context-output", extra={"IMRE_ENCODED_FLAGS": flags})
        assert (result.returncode, result.stdout) == (0, b"written 4\nclosed True\ncontent True\n"), result


def check_fixed_float_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "refuse_fixed_float.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_fixed_float_format.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_float_format,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    source = fixtures / "fixed_float_format_allocation.py"
    for optimization in (0, 3):
        executable = root / f"fixed-float-O{optimization}"
        built = run(imre, "build", source, "--no-cache", "--opt-level", str(optimization), "-o", executable,
                    extra={"IMRE_ENCODED_FLAGS": flags})
        assert built.returncode == 0, built
        for shape in (0, 3, 6):
            for prefix in (0, 1):
                arguments = ("1.25", str(shape), str(prefix))
                reference = run(sys.executable, "-B", source, *arguments)
                assert reference.returncode == 0 and not reference.stderr, reference
                for policy in ("none", "buffer", "owner"):
                    failed = policy == "owner" or (policy == "buffer" and shape != 6)
                    result = run(executable, *arguments, extra={
                        "FIXED_FLOAT_REFUSE": policy,
                        "FIXED_FLOAT_CALL": str(1 + prefix),
                    })
                    expected = b"MemoryError\n" + reference.stdout.partition(b"\n")[2] if failed else reference.stdout
                    assert (result.returncode, result.stdout, result.stderr) == (
                        0, expected, b"fixed_float_allocation_refused\n" if failed else b"",
                    ), (optimization, shape, prefix, policy, result)


def check_boxed_literal_roots_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "literal_roots_refuse.o"
    subprocess.run(["cc", "-c", fixtures / "literal_roots_refuse.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc",
    ])
    source = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_literal_roots as fixture; fixture.allocation()"
    for ordinal in (0, 1):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        expected = (b"caught ()\n" if ordinal else b"value 3\n") + b"retry 3 True\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if ordinal else b"",
        ), (ordinal, result)


def check_select_result_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "refuse_select_result.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_select_result.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_scoped_call_at,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    source = fixtures / "boxed_select_result_allocation.py"
    for optimization in (0, 3):
        executable = root / f"select-result-O{optimization}"
        built = run(imre, "build", source, "--runtime-imports", "--no-cache", "--opt-level", str(optimization), "-o", executable,
                    extra={"IMRE_ENCODED_FLAGS": flags})
        assert built.returncode == 0, built
        for shape in range(4):
            for count in ((1,) if shape == 0 else (1, 511)):
                arguments = (str(shape), str(count))
                reference = run(sys.executable, "-B", source, *arguments)
                assert (reference.returncode, reference.stdout, reference.stderr) == (0, b"ok\nTrue\nTrue\n", b""), reference
                for refused in (0, 1):
                    result = run(executable, *arguments, extra={"SELECT_RESULT_REFUSE": str(refused)})
                    assert (result.returncode, result.stdout, result.stderr) == (
                        0, b"MemoryError\nTrue\n" if refused else reference.stdout,
                        b"select_result_allocation_refused\n" if refused else b"",
                    ), (optimization, shape, count, refused, result)


def check_poll_result_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "refuse_poll_result.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_poll_result.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=poll,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    source = fixtures / "boxed_poll_result_allocation.py"
    for optimization in (0, 3):
        executable = root / f"poll-result-O{optimization}"
        built = run(imre, "build", source, "--runtime-imports", "--no-cache", "--opt-level", str(optimization), "-o", executable,
                    extra={"IMRE_ENCODED_FLAGS": flags})
        assert built.returncode == 0, built
        for count in (1, 2, 7):
            reference = run(sys.executable, "-B", source, str(count), "select")
            assert (reference.returncode, reference.stdout, reference.stderr) == (0, b"ok\nTrue\nTrue\n", b""), reference
            first = 1 + (count == 2)
            for nth in sorted({0, first, first + count - 1}):
                result = run(executable, str(count), "select", extra={"POLL_RESULT_REFUSE_NTH": str(nth)})
                assert (result.returncode, result.stdout, result.stderr) == (
                    0, b"MemoryError\nTrue\n" if nth else reference.stdout,
                    b"poll_result_allocation_refused\n" if nth else b"",
                ), (optimization, count, nth, result)


def check_select_input_root_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "refuse_select_input_roots.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_select_input_roots.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_scoped_call_at,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    source = fixtures / "boxed_select_input_root_allocation.py"
    for optimization in (0, 3):
        executable = root / f"select-input-roots-O{optimization}"
        built = run(imre, "build", source, "--runtime-imports", "--no-cache", "--opt-level", str(optimization), "-o", executable,
                    extra={"IMRE_ENCODED_FLAGS": flags})
        assert built.returncode == 0, built
        for shape in (1, 2):
            arguments = (str(shape), "1023")
            reference = run(sys.executable, "-B", source, *arguments)
            assert (reference.returncode, reference.stdout, reference.stderr) == (0, b"ok\nTrue\nTrue\n", b""), reference
            for size, nth in ((1024, 0), (1024, 2), (8192, 2)):
                result = run(executable, *arguments, extra={"SELECT_ROOT_REFUSE_BYTES": str(size), "SELECT_ROOT_REFUSE_NTH": str(nth)})
                assert (result.returncode, result.stdout, result.stderr) == (
                    0, b"MemoryError\nTrue\n" if nth else reference.stdout,
                    b"select_input_root_allocation_refused\n" if nth else b"",
                ), (optimization, shape, size, nth, result)


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


def check_boxed_json_callback_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "json_callback_refuse.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_json_callback_arguments.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_scoped_call_at,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    imports = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_json_callback_arguments as fixture; "
    source = root / "json_callback_probe.py"
    source.write_text(imports + "fixture.current(int(sys.argv[1]))\n")
    executable = root / "json_callback_probe"
    built = run(imre, "build", source, "--runtime-imports", "--no-cache", "-o", executable, extra={
        "IMRE_ENCODED_FLAGS": flags,
    })
    assert built.returncode == 0, built.stderr
    # Refuse the singleton arguments for default, float repr, writer and sorting.
    # Native allocation traces distinguish these from earlier output-buffer growth.
    for mode in range(5):
        target = (2, 2, 3, 2, 6)[mode]
        for ordinal in (0, target, 37):
            result = run(executable, str(mode), extra={
                "IMRE_TEST_JSON_CALLBACK_NTH": str(ordinal),
            })
            expected = b"MemoryError True\nTrue\n" if ordinal == target else (
                b"ok True\nTrue\n" if mode in (0, 3, 4) else b"ValueError True\nTrue\n"
            )
            assert (result.returncode, result.stdout, result.stderr) == (
                0, expected, b"refused\n" if ordinal == target else b"",
            ), (mode, ordinal, result)



def marked_call_refusal_flags(root):
    fixture = Path(__file__).resolve().with_name("fixtures") / "refuse_marked_call.c"
    wrapper = root / "refuse_marked_call.o"
    subprocess.run(["cc", "-c", fixture, "-o", wrapper], check=True)
    return "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_scoped_call_at,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])


def check_noncallable_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    for shape in [0, 4, 10, 11, 13, 15]:
        for target in [0, 1, "all"]:
            extra = {"IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_REFUSE_NTH": str(target if target != "all" else 0)}
            if target == "all":
                extra["IMRE_TEST_CALL_REFUSE_ALL"] = "1"
            result = run(imre, "-B", fixtures / "boxed_noncallable_allocation.py", str(shape), extra=extra)
            assert result.returncode == 0 and result.stdout.endswith(b"preserved True\nretry True\n"), result
            if target:
                assert result.stdout.startswith(b"kind MemoryError\n"), result
                assert b"call_allocation 1 " in result.stderr and b" 1\n" in result.stderr, result
            else:
                reference = run(sys.executable, "-B", fixtures / "boxed_noncallable_allocation.py", str(shape))
                assert result.stdout == reference.stdout and reference.returncode == 0, (result, reference)


def check_weak_proxy_root_allocation(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "weak_proxy_root_refuse.o"
    subprocess.run(["cc", "-c", fixtures / "refuse_weak_proxy_root.c", "-o", wrapper], check=True)
    flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_op_at,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    source = root / "weak_proxy_subscripts.py"
    # A single expression keeps the reads' temporary handles in the same frame.
    reads = ", ".join(["values[index]"] * 512)
    fixture = (fixtures / "boxed_weak_proxy_root_allocation.py").read_text()
    source.write_text(f"{fixture}\ncheck(lambda values, index: ({reads}))\n")
    reference = run(sys.executable, "-B", source)
    assert reference.returncode == 0, reference
    for ordinal in (0, 1):
        result = run(imre, "-B", source, extra={"IMRE_ENCODED_FLAGS": flags, "PROXY_ROOT_REFUSE_NTH": str(ordinal)})
        expected = reference.stdout if not ordinal else reference.stdout.replace(b"kind ok\n", b"kind MemoryError\n", 1)
        assert result.returncode == 0 and result.stdout == expected, (ordinal, result, reference)
        refused = any(line.startswith(b"weak_proxy_root_allocation ") and line.endswith(b" 1") for line in result.stderr.splitlines())
        assert refused == bool(ordinal), (ordinal, result)


def call_allocation_flags(root):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "itemgetter_refuse.o"
    subprocess.run(["cc", "-c", fixtures / "itemgetter_refuse.c", "-o", wrapper], check=True)
    return "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_scoped_call_at,--wrap=imre_dyn_op_at,--wrap=malloc,--wrap=realloc,--wrap=imre_dyn_lit",
    ])


def check_callable_label_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    for shape in range(10):
        reference = run(sys.executable, "-B", fixtures / "boxed_callable_label_allocation.py", str(shape))
        assert reference.returncode == 0, reference
        for target in [0, 1, 2, "all"]:
            extra = {"IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_REFUSE_NTH": str(target if target != "all" else 0)}
            if target == "all":
                extra["IMRE_TEST_CALL_REFUSE_ALL"] = "1"
            result = run(imre, "-B", fixtures / "boxed_callable_label_allocation.py", str(shape), extra=extra)
            assert result.returncode == 0 and b"preserved True\n" in result.stdout, result
            activated = any(line.startswith(b"call_allocation ") and line.endswith(b" 1") for line in result.stderr.splitlines())
            expected = reference.stdout.replace(b"kind ok\n", b"kind MemoryError\n", 1) if activated else reference.stdout
            assert result.stdout == expected, (result, reference)
            expected_denial = target in (1, "all") or (shape >= 8 and target == 2)
            assert activated == expected_denial, (shape, target, result)
            if activated:
                assert result.stdout.startswith(b"kind MemoryError\n"), result
                assert b"call_allocation " in result.stderr, result


def check_boxed_call_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    import_path = f"import sys; sys.path.insert(0, {str(fixtures)!r}); "
    source = import_path + "import boxed_short_circuit as fixture; fixture.skipped_literal()"
    result = run(imre, "-B", "-c", source, extra={
        "IMRE_ENCODED_FLAGS": flags,
        "IMRE_TEST_CALL_SIZE": "8191",
        "IMRE_TEST_GETTER_ALLOCATION": "1",
    })
    assert (result.returncode, result.stdout, result.stderr) == (0, b"37\n", b""), result

    source = f"import sys; sys.path.insert(0, {str(fixtures)!r}); import boxed_traceback_allocation as fixture; fixture.allocation()"
    for size in (0, 160, 224):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        expected = b"MemoryError () ValueError invalid literal for int() with base 10: 'invalid'\n23\n" if size else b"ValueError\n23\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), result

    source = import_path + "import boxed_memory_error_kind as fixture; fixture.allocation()"
    for ordinal in (0, 1):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": "8",
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
            "IMRE_TEST_MEMORY_ERROR_KIND": "1",
        })
        expected = (b"caught ()\n" if ordinal else b"value 3\n") + b"retry 3 True\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if ordinal else b"",
        ), (ordinal, result)

    source = import_path + "import boxed_call_labels as fixture; fixture.allocation()"
    for size in (0, 152):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": "1",
        })
        expected = (b"caught ()\n" if size else b"value 3\n") + b"retry 3\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), (size, result)

    for kind, size, ordinal in (
        ("plain", 0, 1), ("plain", 56, 3), ("plain", 56, 4),
        ("plain", 96, 1), ("plain", 96, 2), ("plain", 116, 1), ("plain", 24, 1),
        ("posonly", 56, 5), ("nonlocal", 116, 2),
    ):
        source = import_path + f"import boxed_call_parameters as fixture; fixture.allocation({kind!r})"
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        expected = (b"caught ()\n" if size else b"value 3\n") + b"retry 3\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), (size, ordinal, result)

    source = import_path + "import boxed_call_parameters as fixture; fixture.allocation()"
    for size, ordinal in ((0, 1), (56, 5), (52, 1)):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": flags,
            "IMRE_TEST_CALL_SIZE": str(size),
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        expected = (b"caught ()\n" if size else b"value 3\n") + b"retry 3\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if size else b"",
        ), (size, ordinal, result)

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

    for operation in ("lookup", "assign"):
        source = import_path + f"import boxed_identifier_operands as fixture; fixture.{operation}_allocation()"
        for size in (0, 56):
            result = run(imre, "-B", "-c", source, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "IMRE_TEST_CALL_OP": operation,
                "IMRE_TEST_CALL_SIZE": str(size),
                "IMRE_TEST_GETTER_ALLOCATION": "1",
            })
            if operation == "lookup":
                expected = (b"caught ()\n" if size else b"value 3\n") + b"retry 3\n"
            else:
                expected = (b"caught () True\n" if size else b"value 7\n") + b"preserved True\nretry 3\n"
            assert (result.returncode, result.stdout, result.stderr) == (
                0, expected, b"refused\n" if size else b"",
            ), (operation, size, result)

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
    source = import_path + "import boxed_json_allocation as fixture; fixture.output_growth()"
    result = run(imre, "-B", "-c", source, extra={
        "IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_SIZE": "4096", "IMRE_TEST_GETTER_ALLOCATION": "1",
    })
    assert (result.returncode, result.stdout, result.stderr) == (0, b"7386 1231 True\n", b"refused\n"), result
    source = import_path + "import boxed_bytes_add as fixture; fixture.copy_allocation()"
    result = run(imre, "-B", "-c", source, extra={
        "IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_SIZE": "16381", "IMRE_TEST_GETTER_ALLOCATION": "1",
    })
    assert (result.returncode, result.stdout, result.stderr) == (0, b"() 16381 0\n16381\n", b"refused\n"), result
    source = import_path + "import boxed_formatter as fixture; fixture.integer_allocation()"
    result = run(imre, "-B", "-c", source, extra={
        "IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_SIZE": "2466", "IMRE_TEST_GETTER_ALLOCATION": "1",
    })
    assert (result.returncode, result.stdout, result.stderr) == (0, b"caught ()\n8191 2466 True\n", b"refused\n"), result
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

    for kind in ("list", "tuple"):
        source = import_path + f"import boxed_iterator_collection as fixture; fixture.allocation({kind!r})"
        for size in (0, 8192):
            result = run(imre, "-B", "-c", source, extra={
                "IMRE_ENCODED_FLAGS": flags,
                "IMRE_TEST_CALL_SIZE": str(size),
                "IMRE_TEST_GETTER_ALLOCATION": "1",
            })
            expected = (b"caught ()\n" if size else b"value 513 True\n") + b"retry 513\n"
            assert (result.returncode, result.stdout, result.stderr) == (
                0, expected, b"refused\n" if size else b"",
            ), (kind, size, result)

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


def check_bound_method_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    for shape in range(6):
        reference = run(sys.executable, "-B", fixtures / "boxed_bound_method_allocation.py", str(shape))
        assert reference.returncode == 0, reference
        for target in [0, 1, "all"]:
            extra = {"IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_REFUSE_NTH": str(target if target != "all" else 0)}
            if target == "all":
                extra["IMRE_TEST_CALL_REFUSE_ALL"] = "1"
            result = run(imre, "-B", fixtures / "boxed_bound_method_allocation.py", str(shape), extra=extra)
            assert result.returncode == 0 and b"preserved True\n" in result.stdout, result
            expected = reference.stdout if not target else reference.stdout.replace(b"kind ok\n", b"kind MemoryError\n", 1)
            assert result.stdout == expected, (result, reference)
            if target:
                assert result.stdout.startswith(b"kind MemoryError\n"), result
                assert b"call_allocation " in result.stderr and b" 1\n" in result.stderr, result


def check_ffi_result_roots(imre, root, run):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    wrapper = root / "ffi-result-roots.o"
    subprocess.run(["cc", "-c", fixtures / "ffi_result_roots_refuse.c", "-o", wrapper], check=True)
    result_flags = "\x1f".join([
        "-C", f"link-arg={wrapper}",
        "-C", "link-arg=-Wl,--wrap=imre_dyn_op_at,--wrap=imre_dyn_lit,--wrap=malloc,--wrap=realloc,--wrap=calloc",
    ])
    import_path = f"import sys; sys.path.insert(0, {str(fixtures)!r}); "
    source = import_path + "import boxed_ffi_result_roots as fixture; fixture.allocation()"
    for ordinal in (0, 1):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": result_flags,
            "IMRE_TEST_CALL_SIZE": "128",
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        })
        expected = (b"caught ()\n" if ordinal else b"value 3\n") + b"retry 3 True\n"
        assert (result.returncode, result.stdout, result.stderr) == (
            0, expected, b"refused\n" if ordinal else b"",
        ), (ordinal, result)



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


def check_class_metadata_allocation(imre, run, flags):
    fixtures = Path(__file__).resolve().with_name("fixtures")
    for shape in range(7):
        reference = run(sys.executable, "-B", fixtures / "boxed_class_metadata_allocation.py", str(shape))
        assert reference.returncode == 0, reference
        for target in [0, 1, 2, "all"]:
            extra = {"IMRE_ENCODED_FLAGS": flags, "IMRE_TEST_CALL_REFUSE_NTH": str(target if target != "all" else 0)}
            if target == "all":
                extra["IMRE_TEST_CALL_REFUSE_ALL"] = "1"
            result = run(imre, "-B", fixtures / "boxed_class_metadata_allocation.py", str(shape), extra=extra)
            assert result.returncode == 0 and b"metadata True True\n" in result.stdout, result
            expected = reference.stdout if not target else reference.stdout.replace(b"kind ok\n", b"kind MemoryError\n", 1)
            assert result.stdout == expected, (result, reference)
            if target:
                assert result.stdout.startswith(b"kind MemoryError\n"), result
                assert b"call_allocation " in result.stderr and b" 1\n" in result.stderr, result



def check_python_process_identity(imre, root, env):
    import select
    import signal

    source = (
        "import os, sys\n"
        "assert sys.argv[-1] == 'tail' and sys.orig_argv[1] == '-B'\n"
        "try: os.waitpid(-1, os.WNOHANG)\n"
        "except ChildProcessError: pass\n"
        "else: raise AssertionError('launcher left a child')\n"
        "os.write(1, (str(os.getpid()) + '\\n').encode())\n"
        "os.read(0, 1)\n"
    )
    script = root / "process_identity.py"
    script.write_text(source)
    for args, sig, disposition in (
        (["-c", source], signal.SIGTERM, signal.SIG_IGN),
        ([script], signal.SIGKILL, signal.SIG_DFL),
        (["-c", source], signal.SIGINT, signal.SIG_DFL),
    ):
        def before_exec():
            signal.signal(signal.SIGCHLD, disposition)

        with subprocess.Popen(
            [imre, "-B", *args, "tail"], env=env, cwd=root, start_new_session=True,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, preexec_fn=before_exec,
        ) as child:
            try:
                assert select.select([child.stdout], [], [], 600)[0], "payload did not report its PID"
                got = child.stdout.readline()
                assert got == f"{child.pid}\n".encode(), (got, child.poll())
                if sys.platform.startswith("linux"):
                    # The compiler temporarily needs waitable children for its linker.
                    lines = Path(f"/proc/{child.pid}/status").read_text().splitlines()
                    ignored = int(next(line.split()[1] for line in lines if line.startswith("SigIgn:")), 16)
                    assert bool(ignored & (1 << (signal.SIGCHLD - 1))) == (disposition == signal.SIG_IGN)
                child.send_signal(sig)
                assert child.wait(timeout=10) == -sig
            finally:
                # Also release a payload left behind by a regressed wrapper.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


def check_os_abort(imre, run):
    import signal

    source = (
        "import os, resource, signal\n"
        "resource.setrlimit(resource.RLIMIT_CORE, (0, 0))\n"
        "signal.signal(signal.SIGABRT, signal.SIG_IGN)\n"
        "os.abort()\n"
    )
    result = run(imre, "-c", source, timeout=600)
    assert result.returncode == -signal.SIGABRT, result


def check_faulthandler(imre, root, env):
    import resource
    import select
    import signal

    source = root / "fault.py"
    source.write_text(
        "import sys\n"
        "assert 'faulthandler' in sys.modules\n"
        "import faulthandler, os\n"
        "assert sys._xoptions == {'faulthandler': '0'} and faulthandler.is_enabled()\n"
        "if 'disable' in sys.argv: assert faulthandler.disable()\n"
        "def wait_for_signal():\n"
        "    os.write(1, (str(os.getpid()) + '\\n').encode())\n"
        "    input()\n"
        "wait_for_signal()\n"
    )

    def no_core():
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

    cmd = [imre, "-X", "faulthandler=0", "-I", source]
    for send, args in ((os.kill, []), (os.killpg, []), (os.kill, ["disable"])):
        # Keep the parent writer open while communicate() drains the traceback.
        reader, writer = os.pipe()
        with (
            os.fdopen(reader, "rb") as stdin,
            os.fdopen(writer, "wb"),
            subprocess.Popen(
                [*cmd, *args], cwd=root, env=env, preexec_fn=no_core, start_new_session=True,
                stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            ) as child,
        ):
            try:
                assert select.select([child.stdout], [], [], 600)[0], "interpreter did not report its PID"
                reported = os.read(child.stdout.fileno(), 64)
                assert reported == f"{child.pid}\n".encode(), (reported, child.pid)
                send(child.pid, signal.SIGABRT)
                out, err = child.communicate(timeout=10)
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
        assert child.returncode == -signal.SIGABRT and not out, (child.returncode, out, err)
        if args:
            assert not err, err
        else:
            assert b"Fatal Python error: Aborted" in err, err
            assert f'File "{source}", line 9 in <module>\n'.encode() in err, err
            # The signal may arrive during the PID write or the blocking input.
            assert any(f'File "{source}", line {n} in wait_for_signal\n'.encode() in err for n in (7, 8)), err


def check_pending_signals(imre, root, env):
    import signal

    fixture = Path(__file__).resolve().with_name("fixtures") / "boxed_signal_pending.py"
    native = root / "signal-pending"
    subprocess.run([imre, "build", fixture, "--runtime-imports", "-o", native], check=True, env=env)
    for delivered, expected in (
        (0, 0),
        (signal.SIGUSR1, signal.SIGUSR1),
        (signal.SIGUSR2, signal.SIGUSR2),
        (signal.SIGRTMIN + 1, signal.SIGRTMIN + 1),
        (signal.SIGUSR2, signal.SIGUSR1),
    ):
        def queue_before_exec():
            if delivered:
                signal.pthread_sigmask(signal.SIG_BLOCK, {delivered})
                os.kill(os.getpid(), delivered)

        result = subprocess.run(
            [native, str(int(expected))], env=env, capture_output=True,
            preexec_fn=queue_before_exec, timeout=30,
        )
        if delivered == expected:
            assert result.returncode == 0 and result.stderr == b"", result
            assert result.stdout == f"verified pending signals {int(expected)}\n".encode(), result
        else:
            assert result.returncode == 1 and b"unexpected pending signals" in result.stderr, result


def check_marshal_allocation(exe):
    import resource
    import select

    with subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE) as child:
        try:
            assert select.select([child.stdout], [], [], 30)[0]
            assert child.stdout.readline() == b"ready\n"
            pages = int(Path(f"/proc/{child.pid}/statm").read_text().split()[0])
            cap = pages * os.sysconf("SC_PAGESIZE") + 512 * 1024
            resource.prlimit(child.pid, resource.RLIMIT_AS, (cap, cap))
            out, err = child.communicate(b"x", timeout=30)
        finally:
            child.kill()
    assert (child.returncode, out, err) == (0, b"refused\nTrue\n", b""), (out, err)


def check_pickle_reductions(imre, root, run):
    import pickle

    fixture = Path(__file__).resolve().with_name("fixtures") / "boxed_pickle_reduction.py"
    work = root / "pickle-reductions"
    work.mkdir()
    (work / fixture.name).write_bytes(fixture.read_bytes())
    entry = work / "main.py"
    sys.path.insert(0, str(work))  # CPython must import the reconstruction function by name.
    try:
        from boxed_pickle_reduction import LabeledTuple

        obj = LabeledTuple((3, 4))
        obj.label = {"back": obj}
        streams = [pickle.dumps((obj, obj.label), protocol) for protocol in range(6)]
        entry.write_text(
            "from boxed_pickle_reduction import consume, produce\n"
            f"consume({streams!r})\nproduce()\n"
        )
        for optimization in (0, 3):
            executable = work / f"pickle-O{optimization}"
            build = run(imre, "build", entry, "--module-root", work, "--runtime-imports",
                        "--no-cache", "--opt-level", str(optimization), "-o", executable)
            assert build.returncode == 0, build
            result = run(executable)
            assert (result.returncode, result.stderr) == (0, b""), result
            records = result.stdout.splitlines()
            assert len(records) == 6, result
            for protocol, record in enumerate(records):
                data = bytes(json.loads(record))
                if protocol >= 2:
                    assert data.startswith(bytes([0x80, protocol])), data
                value = pickle.loads(data)
                shared = ("\0\n\r\\\x1aéĀ🍏", 1 << 2048, -2147483649, True, None)
                assert value[:6] == (shared, shared, ("rebuilt", protocol), ("rebuilt", protocol), "37", ())
                assert value[0] is value[1] and value[2] is value[3]
                payload = bytes(range(256))
                assert value[7] == (b"", payload[:-1], payload, payload, ("rebuilt", protocol))
                assert value[7][2] is value[7][3]
                mapping = value[9]
                assert list(mapping) == ["z", "a", "m"] and mapping["m"] == 42
                assert mapping["z"] is mapping and mapping.state is mapping["a"]
                singleton, live, pair, iterator = value[8]
                assert singleton == ([7] if protocol else [7, "added"])
                assert live == [7, "second", "added"]
                assert pair[0] is pair[1] and pair[0][0] is pair[0]
                assert list(iterator) == ["c", "a"]
                graph, small, wide, replaced, recursive = value[6]
                assert graph["self"] is graph and graph["left"] is graph["right"]
                assert graph["distinct"] == graph["left"] == {} and graph["distinct"] is not graph["left"]
                assert small[0]["back"] is small and wide[0]["back"] is wide
                assert recursive[0]["back"] is recursive
                assert list(replaced.items()) == [("first", 5), ("later", 37)]
    finally:
        sys.path.pop(0)


def check_boxed_unittest_main_reporting(imre, root, run):
    source = root / "reporting_tests.py"
    source.write_text(
        "import importlib\n"
        "unittest = importlib.import_module('unittest')\n"
        "class Cases(unittest.TestCase):\n"
        "    def test_pass(self): pass\n"
        "    def test_failure(self):\n"
        "        with self.subTest(item=1): self.fail('intentional failure')\n"
        "unittest.main()\n"
    )
    executable = root / "reporting_tests"
    built = run(imre, "build", source, "--runtime-imports", "--no-cache", "-o", executable)
    assert built.returncode == 0, built
    passed = run(executable, "-v", "Cases.test_pass")
    assert passed.returncode == 0, passed
    assert b"test_pass (__main__.Cases.test_pass) ... ok" in passed.stderr.splitlines(), passed
    failed = run(executable, "-v", "Cases.test_failure")
    assert failed.returncode == 1 and b"(item=1) ... FAIL" in failed.stderr, failed
    assert failed.stderr.rstrip().endswith(b"FAILED (failures=1)"), failed


def check_unittest_main_names(imre, root, run):
    source = root / "selected_tests.py"
    source.write_text(
        "import sys\n"
        "import unittest\n"
        "class Cases(unittest.TestCase):\n"
        "    def test_a(self): print('a')\n"
        "    def test_b(self): print('b')\n"
        "assert sys.modules['__main__'].Cases is Cases\n"
        "unittest.main()\n"
    )
    executable = root / "selected_tests"
    built = run(imre, "build", source, "--module-root", root, "--runtime-imports", "--no-cache", "-o", executable)
    assert built.returncode == 0, built
    for args, output in [((), b"a\nb\n"), (("Cases.test_b", "Cases.test_a"), b"b\na\n"),
                         (("-vk", "test_b", "Cases"), b"b\n")]:
        result = run(executable, *args)
        assert result.returncode == 0 and result.stdout == output, result
    for flags, progress in [((), b".\n"), (("-v",), b"test_b (__main__.Cases.test_b) ... ok\n\n"),
                            (("-q",), b""), (("-vq",), b"")]:
        result = run(executable, *flags, "Cases.test_b")
        assert result.returncode == 0 and result.stdout == b"b\n", result
        assert result.stderr.split(b"-" * 70, 1)[0] == progress, result
    extra = run(executable, "Cases", "-v", "Cases.test_b")
    assert extra.returncode == 2 and not extra.stdout and b"unrecognized arguments" in extra.stderr, extra
    for flags in [("-k", "-1"), ("-k=two words",)]:
        filtered = run(executable, *flags, "Cases")
        assert filtered.returncode == 5 and not filtered.stdout, filtered
    missing = run(executable, "-v", "Cases.test_missing")
    assert missing.returncode == 1 and b"ERROR: test_missing (unittest.loader._FailedTest.test_missing)" in missing.stderr, missing
    assert b"Ran 1 test" in missing.stderr and b"FAILED (errors=1)" in missing.stderr, missing


def check_unittest_file_discovery(imre, root, run):
    entry = root / "discover_after_build.py"
    entry.write_text(
        "import sys, unittest\n"
        "suite = unittest.TestLoader().discover(sys.argv[1])\n"
        "result = unittest.TestResult()\n"
        "suite.run(result)\n"
        "print(result.testsRun, [test.id() for test, error in result.failures], len(result.errors))\n"
        "print([(test.id(), reason) for test, reason in result.skipped])\n"
    )
    for optimization in ("0", "3"):
        directory = root / ("late_tests_" + optimization)
        directory.mkdir()
        executable = root / ("discover_" + optimization)
        built = run(imre, "build", entry, "--runtime-imports", "--no-cache",
                    "--opt-level", optimization, "-o", executable)
        assert built.returncode == 0, built
        # The compiler has already linked the binary when this module first appears.
        (directory / "test_late.py").write_text(
            "import unittest\n"
            "from enum import Enum\n"
            "class Marker(Enum):\n"
            "    present = 1\n"
            "class Late(unittest.TestCase):\n"
            "    def test_body(self):\n"
            "        for member in Marker: self.fail(member.name)\n"
            "    def test_util(self):\n"
            "        from unittest.util import safe_repr\n"
            "        self.assertIs(safe_repr, unittest.util.safe_repr)\n"
            "        self.assertEqual(safe_repr(None), 'None')\n"
        )
        (directory / "test_skipped.py").write_text(
            "import unittest\nraise unittest.SkipTest('unavailable module')\n"
        )
        (directory / "test_process_exports.py").write_text(
            "import subprocess as _process, unittest as _unittest\n"
            "from subprocess import __all__ as _exports\n"
            "from subprocess import *\n"
            "_names = []\n"
            "for _name in list(globals()):\n"
            "    if not _name.startswith('_'):\n"
            "        _names.append(_name)\n"
            "class Exports(_unittest.TestCase):\n"
            "    def test_star(self):\n"
            "        self.assertEqual(_names, _exports)\n"
            "        for name in _exports:\n"
            "            self.assertIs(globals()[name], getattr(_process, name))\n"
            "        self.assertEqual(check_output(['/bin/sh', '-c', 'printf captured']), b'captured')\n"
        )
        for command in ((sys.executable, "-B", entry), (executable,)):
            result = run(*command, directory)
            assert (result.returncode, result.stdout, result.stderr) == (
                0, b"4 ['test_late.Late.test_body'] 0\n"
                b"[('unittest.loader.ModuleSkipped.test_skipped', 'unavailable module')]\n", b"",
            ), result


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

        def run(exe, *args, extra=None, stdin=None, timeout=60):
            return subprocess.run(
                [exe, *args],
                cwd=root,
                env=env | (extra or {}),
                input=stdin,
                capture_output=True,
                check=False,
                timeout=timeout,
            )

        if sys.platform == "darwin" or sys.platform.startswith("linux"):
            check_faulthandler(imre, root, env)
            check_python_process_identity(imre, root, env)
        if sys.platform.startswith("linux"):
            check_os_abort(imre, run)
            check_pending_signals(imre, root, env)
            marked_call_flags = marked_call_refusal_flags(root)
            check_bound_method_allocation(imre, run, marked_call_flags)
            check_sysconfig_build(imre, root, run)
            check_ffi_result_roots(imre, root, run)
            check_weak_proxy_root_allocation(imre, root, run)
            check_noncallable_allocation(imre, run, marked_call_flags)
            check_select_result_allocation(imre, root, run)
            check_select_input_root_allocation(imre, root, run)
            check_poll_result_allocation(imre, root, run)
            flags = call_allocation_flags(root)
            check_callable_label_allocation(imre, run, marked_call_flags)
            check_argument_allocation(imre, run, flags)
            check_class_metadata_allocation(imre, run, marked_call_flags)
            check_raise_site_allocation(imre, root, run)
            check_io_compaction_allocation(imre, root, run)
            check_context_protocol_allocation(imre, root, run)
            check_fixed_float_allocation(imre, root, run)
            check_boxed_literal_roots_allocation(imre, root, run)
            check_boxed_call_allocation(imre, run, flags)
        check_boxed_json_callback_allocation(imre, root, run)
        check_pickle_reductions(imre, root, run)

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

        spec_entry = root / "runtime_spec.py"
        spec_entry.write_text(
            "from importlib.machinery import ModuleSpec\n"
            "try:\n    import missing_optional_module\n"
            "except ModuleNotFoundError:\n"
            "    spec = ModuleSpec('pkg', None, origin='memory', is_package=True)\n"
            "    print(spec.name, spec.origin, spec.submodule_search_locations)\n"
        )
        opted_in = run(imre, "run", spec_entry, "--runtime-imports", "--quiet")
        assert (opted_in.returncode, opted_in.stdout) == (0, b"pkg memory []\n"), opted_in.stderr

        (root / "reader.py").write_text(
            "for _ in range(3):\n    print(repr(input()))\n"
            "try:\n    input()\nexcept EOFError:\n    print('EOF')\n"
        )
        for exe in (sys.executable, imre):
            actual = run(exe, "-B", "-c", "import reader", stdin="one\r\n\nλ".encode())
            assert actual.returncode == 0 and not actual.stderr, actual.stderr
            assert actual.stdout == "'one\\r'\n''\n'λ'\nEOF\n".encode()

        check_command_stdin(imre, run)

        # The binary buffer must work from an ordinary -c command on its own.
        for stream in ("stdout", "stderr"):
            command = (
                f"import sys; output = sys.{stream}; buf = output.buffer;"
                "buf.write(b'\\xff\\r\\n'); buf.flush()"
            )
            actual = run(imre, "-c", command)
            expected = run(sys.executable, "-c", command)
            assert actual.returncode == expected.returncode == 0, actual.stderr
            assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr)

        streams = root / "streams.py"
        streams.write_bytes((Path(__file__).with_name("fixtures") / "boxed_sys_output.py").read_bytes())
        expected = run(sys.executable, "-B", streams)
        actual = run(imre, "run", streams, "--runtime-imports", "--quiet")
        assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
        assert actual.stdout == expected.stdout == "stdout π\nraw:then:text\n".encode()
        assert actual.stderr == expected.stderr == "stderr 🌍\nrestored\nbinary stderr\n".encode()

        text_close = run(imre, "-B", "-c", "import streams", "close-text")
        assert text_close.returncode == 0, text_close.stderr
        assert (text_close.stdout, text_close.stderr) == (expected.stdout, expected.stderr)

        chained = root / "chained.py"
        chained.write_text(
            "import sys\n"
            "sys.tracebacklimit = 0\n"
            "try:\n"
            "    try:\n"
            "        raise ValueError('inner')\n"
            "    except ValueError:\n"
            "        raise RuntimeError('middle')\n"
            "except RuntimeError as exc:\n"
            "    raise OSError(28, 'outer') from exc\n"
        )
        expected = run(sys.executable, "-B", chained)
        actual = run(imre, "run", chained, "--runtime-imports", "--quiet")
        assert expected.returncode == actual.returncode == 1
        assert actual.stderr == expected.stderr, (actual.stderr, expected.stderr)

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

        def compare(*args, extra=None, status=0, stderr=b"", native_args=None, native_timeout=60):
            expected = run(sys.executable, *args, extra=extra)
            actual = run(imre, *(args if native_args is None else native_args), extra=extra, timeout=native_timeout)
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
            assert actual.stderr == expected.stderr == stderr, (
                args,
                actual.stderr,
                expected.stderr,
            )

        module_pkg = root / "module_pkg"
        module_pkg.mkdir()
        (module_pkg / "__init__.py").write_text("import sys\nassert sys.argv[0] == '-m'\n")
        (module_pkg / "child.py").write_text(
            "import os, sys\n"
            "from . import __path__\n"
            "assert __package__ == 'module_pkg' and __spec__.name == 'module_pkg.child'\n"
            "assert __file__ == sys.argv[0] == __spec__.origin and __loader__ is __spec__.loader\n"
            "assert sys.path[0] == os.getcwd() and __spec__.name not in sys.modules\n"
            "assert sys.modules['__main__'].__dict__ is globals()\n"
            "print(sys.argv[1:], sys.orig_argv[1:])\n"
        )
        (module_pkg / "__main__.py").write_text(
            "import sys\nassert __spec__.name == 'module_pkg.__main__'\n"
            "assert __name__ == '__main__' and __file__ == sys.argv[0]\n"
            "print(__package__, sys.argv[1:])\n"
        )
        compare("-Bm", "module_pkg.child", "-m", "tail")
        compare("-B", "-mmodule_pkg", "tail")

        json_input = root / "input.json"
        json_input.write_text('{"items": [1, 2]}')
        for module in ("json", "json.tool"):
            compare("-B", "-m", module, "--compact", json_input)

        entry = root / "entry"
        entry.mkdir()
        (entry / "__main__.py").write_text(
            "import sys, sibling\n"
            "assert sys.modules['__main__'].__dict__ is globals()\n"
            "assert __spec__.name == '__main__' and __loader__ is __spec__.loader\n"
            "print(sys.argv, sys.path[0], __file__, __package__, sibling.marker)\n"
        )
        sibling = entry / "sibling.py"
        sibling.write_text("marker = 'first'\n")
        compare("entry", "tail")
        (root / "alias").symlink_to(entry, target_is_directory=True)
        compare("-I", "./alias/", "-h")
        sibling.write_text("marker = 'second'\n")
        compare("-I", "./alias/", "-h")
        (root / "empty_entry").mkdir()
        for exe in (sys.executable, imre):
            missing = run(exe, "-I", "empty_entry")
            message = f"can't find '__main__' module in '{root / 'empty_entry'}'"
            assert missing.returncode == 1 and missing.stderr.decode().rstrip().endswith(message)

        archive = root / "application"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as app:
            for prefix in ("", "sub/"):
                app.writestr(prefix + "__main__.py", (entry / "__main__.py").read_text())
                app.writestr(prefix + "sibling.py", f"marker = {prefix!r}\n")
        # A .pyc operand is never Python source, even when its bytes form a valid script.
        bytecode = root / "not_source.pyc"
        bytecode.write_text("print('executed as source')\n")
        compare(bytecode, status=1, stderr=b"RuntimeError: Bad magic number in .pyc file\n")
        bytecode.write_bytes(b"")
        compare(bytecode, status=1, stderr=b"EOFError: EOF read where not expected\n")
        if sys.version_info[:2] == (3, 14):
            renamed = root / "bad_bytecode"
            renamed.write_bytes(importlib.util.MAGIC_NUMBER[:2] + b"xx")
            compare(renamed, status=1, stderr=b"RuntimeError: Bad magic number in .pyc file\n")

        compare("application", "tail")
        compare("-I", "./application/sub", "-h")

        # An ordinary module under the first root wins over a package in a later root.
        (root / "preferred.py").write_text("print('file wins')\n")
        (root / "preferred").mkdir()
        (root / "preferred" / "__main__.py").write_text("invalid syntax!\n")
        later = root / "later" / "preferred"
        later.mkdir(parents=True)
        (later / "__init__.py").write_text("invalid syntax!\n")
        compare("-B", "-m", "preferred", extra={"PYTHONPATH": str(later.parent)})

        helper = root / "request_support.py"
        helper.write_text("from importlib.machinery import ModuleSpec\nspec = ModuleSpec('sample', None)\n")
        request_script = root / "request_entry.py"
        request_script.write_text(
            "from urllib import request\n"
            "from request_support import spec\n"
            "print(request.Request('data:,ok').type, spec.name)\n"
        )
        compare("-B", request_script)

        parser_script = root / "parser_entry.py"
        parser_script.write_text(
            "from argparse import ArgumentParser\n"
            "parser = ArgumentParser()\n"
            "parser.add_argument('value', type=int)\n"
            "print(parser.parse_args().value + 1)\n"
        )
        compare("-I", parser_script, "-h")
        compare("-B", parser_script, "41")

        ftp_script = root / "ftp_entry.py"
        ftp_script.write_text("import ftplib\nprint(ftplib.FTP().encoding, ftplib.parse257('257 \"home\"'))\n")
        compare("-B", ftp_script)

        # A project FTP provider is loaded only if the opener actually uses FTP.
        alternate = root / "ftp_provider"
        alternate.mkdir()
        (alternate / "ftplib.py").write_text("raise RuntimeError('project ftp provider')\n")
        indirect = alternate / "opener.py"
        indirect.write_text(
            "from urllib import request\n"
            "print(request.Request('data:,ok').type)\n"
            "try: request.build_opener(request.ProxyHandler({})).open('ftp:///file')\n"
            "except RuntimeError as error: print(str(error))\n"
        )
        compare("-B", indirect)

        decorated = root / "method_decorators.py"
        decorated.write_text(
            "def prefix(tag):\n"
            "    print('evaluate', tag)\n"
            "    def decorate(method):\n"
            "        print('apply', tag)\n"
            "        return lambda self: tag + method(self)\n"
            "    return decorate\n"
            "\n"
            "class Example:\n"
            "    @prefix('outer/')\n"
            "    @prefix('inner/')\n"
            "    def value(self):\n"
            "        return 'method'\n"
            "\n"
            "print(Example().value())\n"
        )
        compare("-IB", decorated)

        tuple_dir = root / "tuple_dir.py"
        for source in (
            "print(dir((1, 2)))",
            "from collections import namedtuple\nT = namedtuple('T', 'x')\nprint('x' in dir(T(1)))",
        ):
            tuple_dir.write_text(source + "\n")
            compare("-B", tuple_dir, native_args=("run", tuple_dir, "--quiet"))

        unrelated = root / "unrelated_asdict.py"
        unrelated.write_text(
            "from collections import namedtuple\n"
            "Point = namedtuple('Point', 'x y')\n"
            "class Other:\n"
            "    def _asdict(self): return 37\n"
            "def read(other: Other):\n"
            "    saved = other._asdict\n"
            "    return other._asdict(), saved()\n"
            "print(read(Other()))\n"
        )
        compare("-B", unrelated, native_args=("run", unrelated, "--quiet"))

        locale_script = root / "get_locale.py"
        locale_script.write_text(
            "from _locale import LC_ALL, setlocale\n"
            "from locale import getlocale\n"
            "setlocale(LC_ALL, 'C')\n"
            "print(getlocale(), getlocale(category=LC_ALL))\n"
        )
        compare("-B", locale_script)

        years = root / "calendar_years.py"
        years.write_text(
            "from datetime import MINYEAR, date\n"
            "from _datetime import MAXYEAR, datetime\n"
            "assert date(MINYEAR, 1, 1).year == 1\n"
            "assert datetime(MAXYEAR, 12, 31).year == 9999\n"
        )
        compare("-IB", years, native_args=("run", years, "--quiet"))

        arithmetic = root / "date_arithmetic.py"
        for expression in (
            "(day - date(2000, 2, 28)).days",
            "day.__add__(timedelta(days=1))",
            "date.__rsub__(day, date(2000, 3, 1))",
            "getattr(day, '__radd__')(timedelta(days=1))",
        ):
            arithmetic.write_text(
                "from datetime import date, timedelta\n"
                "day = date(2000, 2, 29)\n"
                f"print({expression})\n"
            )
            compare("-IB", arithmetic, native_args=("run", arithmetic, "--quiet"))

        names = root / "calendar_type_names.py"
        names.write_text(
            "from datetime import date, datetime, timedelta\n"
            "print(date.__module__, datetime.__qualname__, timedelta.__module__)\n"
        )
        compare("-IB", names, native_args=("run", names, "--quiet"))

        comparisons = root / "calendar_comparison.py"
        for expression in (
            "d.__eq__(dt)", "date.__le__(dt, d)", "dt.__ne__(d)",
            "getattr(d, '__eq__')(dt)", "getattr(dt, '__ne__')(d)",
        ):
            comparisons.write_text(
                "from datetime import date, datetime\n"
                "d = date(2024, 2, 29)\n"
                "dt = datetime(2024, 2, 29, 23)\n"
                f"print({expression})\n"
            )
            compare("-IB", comparisons, native_args=("run", comparisons, "--quiet"))

        if os.name == "posix":
            compare(
                "-IB", "-c",
                "import sys; os = __import__(sys.argv[1]); posix = __import__(sys.argv[2])\n"
                "f = os.getpid; pid = f()\n"
                "print(type(pid) is int, pid > 0, pid == posix.getpid(), f is posix.getpid)\n"
                "print(f.__module__, f.__name__, f.__qualname__, f.__self__ is posix, repr(f))\n"
                "print(f.__doc__, f.__text_signature__)\n"
                "for args, kw in (((1,), {}), ((), {'x': 1}), ((1,), {'x': 1})):\n"
                "    try: f(*args, **kw)\n"
                "    except TypeError as e: print(str(e))",
                "os", "posix",
            )

            compare(
                "-Xwarn_default_encoding", "-c",
                "from subprocess import *\n"
                "import warnings\n"
                "with warnings.catch_warnings(record=True) as caught:\n"
                "    Popen(['/bin/true'], text=True).wait()\n"
                "    run(['/bin/true'], text=True)\n"
                "    check_output(['/bin/true'], text=True)\n"
                "    run(['/bin/true'], encoding='ascii')\n"
                "    run(['/bin/true'])\n"
                "for w in caught: print(w.filename, w.lineno, w.category.__name__, str(w.message))",
            )

            for statement, connect in (
                ("import socket as net", "net.create_connection"),
                ("from socket import create_connection as dial", "dial"),
            ):
                compare(
                    "-c", f"{statement}\n"
                    "import socket\n"
                    "with socket.socket() as listener:\n"
                    "    listener.bind(('127.0.0.1', 0))\n"
                    "    address = listener.getsockname()\n"
                    f"    try: {connect}(address, timeout=2)\n"
                    "    except ConnectionRefusedError: pass\n"
                    "    else: raise AssertionError('unopened listener accepted a connection')\n"
                    "    listener.listen()\n"
                    f"    with {connect}(address, timeout=2) as client:\n"
                    "        peer, _ = listener.accept()\n"
                    "        with peer:\n"
                    "            client.sendall(b'x')\n"
                    "            assert peer.recv(1) == b'x'\n",
                )

        for mode in ("0", "1"):
            compare(
                "-I", "-X", f"utf8={mode}", "-c",
                "import io; print(io.text_encoding(None))",
            )
        compare(
            "-I", "-X", "warn_default_encoding", "-W", "error::EncodingWarning", "-c",
            "from _io import text_encoding\n"
            "try: text_encoding(None)\n"
            "except EncodingWarning as exc: print(str(exc))",
        )

        compare(
            "-X", "warn_default_encoding", "-Walways", "-c",
            "from io import text_encoding\n"
            "import io, sys, warnings\n"
            "def hook(message, category, filename, lineno, file, line):\n"
            "    print(type(message) is category is EncodingWarning, str(message), filename, lineno, file, line)\n"
            "    raise OSError('from hook')\n"
            "original = sys.stderr\n"
            "sys.stderr = io.StringIO()\n"
            "text_encoding(None, 1)\n"
            "print(sys.stderr.getvalue().strip())\n"
            "sys.stderr = original\n"
            "warnings.showwarning = hook\n"
            "try: text_encoding(None, 1)\n"
            "except OSError as exc: print(exc)\n"
            "warnings.showwarning = 42\n"
            "try: text_encoding(None, 1)\n"
            "except TypeError as exc: print(exc)",
        )
        compare(
            "-X", "warn_default_encoding", "-Werror::EncodingWarning", "-c",
            "from io import text_encoding\n__name__ = None\nprint(text_encoding(None, 1))",
        )

        subset_method = root / "set_issubset.py"
        for statement in (
            "print({1, 2}.issubset([2, 1]))",
            "print(set().issubset([]), frozenset().issubset([]))",
            "items = set([1, 2])\nprint(items.issubset([2, 1]))",
            "items = frozenset({1, 2})\ncheck = items.issubset\nprint(check([1]))",
            "check = {1, 2}.issubset\nprint(check([1]))",
            "print(set.issubset({1}, [1, 2]))",
            "check = set.issubset\nprint(check({2}, [1]))",
            "print(frozenset(['a']).issubset(['a', 'b']))",
            "check = frozenset(['a']).issubset\nprint(check(['b']))",
            "print(frozenset.issubset(frozenset([1]), [1]))",
            "check = frozenset.issubset\nprint(check(frozenset([2]), [1]))",
        ):
            subset_method.write_text(statement + "\n")
            compare("-B", subset_method, native_args=("run", subset_method, "--quiet"))

        set_operator = root / "set_operator.py"
        for statement in (
            "print(set.__sub__({1, 2}, {2}) == {1})",
            "intersection = frozenset.__rand__\nprint(intersection(frozenset(['a']), {'a'}))",
            "items = {1}\nmerge = items.__ior__\nprint(merge({2}) is items)",
        ):
            set_operator.write_text(statement + "\n")
            compare("-B", set_operator, native_args=("run", set_operator, "--quiet"))

        set_method = root / "set_difference_update.py"
        for statement in (
            "items.difference_update([2])",
            "set.difference_update(items, [2])",
            "remove = items.difference_update\nremove([2])",
            "remove = set.difference_update\nremove(items, [2])",
        ):
            set_method.write_text("items = {1, 2}\nalias = items\n" + statement + "\nprint(sorted(alias))\n")
            compare("-B", set_method, native_args=("run", set_method, "--quiet"))

        instance_check = root / "unittest_instance.py"
        for statement in (
            "case.assertIsSubclass(bool, int)",
            "check = Case.assertNotIsSubclass\ncheck(case, int, str)",
            "case.assertIsInstance(1, int)",
            "case.assertNotHasAttr(Case, 'absent')",
            "case.assertNotStartsWith('abc', 'bc')",
            "check = case.assertNotEndsWith\ncheck('abc', 'ab')\n"
            "try: check('abc', ('ab', 'c'))\nexcept AssertionError as exc: print(str(exc))",
            "case.assertSequenceEqual([1, 2], (1, 2))\n"
            "try: case.assertSequenceEqual([1, 2], [1])\nexcept AssertionError as exc: print(str(exc))",
            "Case.assertNotIsInstance(case, 1, str)",
            "check = case.assertIsInstance\ncheck(1, int)",
            "check = Case.assertNotIsInstance\ncheck(case, 1, str)",
        ):
            instance_check.write_text(
                "from unittest import TestCase\nclass Case(TestCase): pass\ncase = Case()\n"
                + statement + "\nprint('checked')\n"
            )
            compare("-B", instance_check, native_args=("run", instance_check, "--quiet"))

        registration = root / "unittest_registration.py"
        registration.write_text(
            "from unittest import TestResult, registerResult, removeResult\n"
            "result = TestResult()\nregisterResult(result)\n"
            "print(removeResult(result), removeResult(result))\n"
        )
        compare("-B", registration, native_args=("run", registration, "--quiet"))

        temporary_read = root / "temporary_readinto.py"
        temporary_read.write_text(
            "from tempfile import TemporaryFile\n"
            "with TemporaryFile() as stream:\n"
            "    stream.write(b'abc')\n"
            "    stream.seek(0)\n"
            "    readinto = stream.readinto\n"
            "    target = bytearray(b'?????')\n"
            "    print(readinto(target), bytes(target), stream.tell())\n"
        )
        compare("-B", temporary_read, native_args=("run", temporary_read, "--quiet"))

        array_import = root / "array_import.py"
        for statement in (
            "from array import array as cls",
            "import array\ncls = array.array",
            'from importlib import import_module\ncls = import_module("array").array',
            'from importlib import import_module\ncls = import_module("".join(("ar", "ray"))).array',
            'cls = __import__("".join(("ar", "ray"))).array',
        ):
            array_import.write_text(
                statement + '\nvalues = cls("i", [7])\nprint(type(values) is cls, list(values))\n'
            )
            compare("-B", array_import, native_args=("run", array_import, "--runtime-imports", "--quiet"))

        hash_args = ["", b"", "latin1é", b"latin1\xe9", "Āfoo", "😀abc", bytes(range(32))]
        hash_calls = "\n".join(f"print(hash({value!r}))" for value in hash_args)
        for seed in ("0", "1", "4294967295"):
            for prefix in ("", 'import sys; sys.path.append("unused")\n'):
                compare("-B", "-c", prefix + hash_calls, extra={"PYTHONHASHSEED": seed})

        if os.name == "posix":
            compare(
                "-IB", "-c",
                "import sys; os = __import__(sys.argv[1]); posix = __import__(sys.argv[2])\n"
                "text = os.getcwd(); raw = os.getcwdb()\n"
                "print(type(text) is str, type(raw) is bytes, text, raw)\n"
                "print(posix.getcwd is os.getcwd, posix.getcwdb is os.getcwdb)\n"
                "for f in (posix.getcwd, os.getcwdb):\n"
                "    try: f(0)\n"
                "    except TypeError as e: print(str(e))\n"
                "    try: f(bad=True)\n"
                "    except TypeError as e: print(str(e))",
                "os", "posix",
            )

        compare(
            "-IB", "-c",
            "import sys; m = __import__(sys.argv[1]); print(m.tok_name[m.EXACT_TOKEN_TYPES[':=']])",
            "token",
        )
        compare("-IB", "-c", "import sys; print(__import__(sys.argv[1]).SelectorKey('f', 7, 1, 'd'))", "selectors")
        compare("-IB", "-c", "load = __import__; print(load('selectors').EVENT_READ)")
        compare(
            "-IB", "-c",
            "import copyreg\n"
            "from copyreg import dispatch_table, pickle\n"
            "pickle(bytes, tuple)\n"
            "print(dispatch_table is copyreg.dispatch_table, dispatch_table[bytes] is tuple)\n"
            "try: pickle(bytes, None)\n"
            "except TypeError as error: print(error)\n",
        )
        compare(
            "-IB", "-c", "import sys; sys.path; m = __import__(sys.argv[1]); "
            "sys.version = '7.42 (custom) [cc]'; print(m.python_version())", "platform",
        )

        compare(
            "-IB", "-c",
            "import sysconfig, textwrap\n"
            "from textwrap import dedent, indent\n"
            "print(repr(dedent(text='  one\\n    two\\n')))\n"
            "print(repr(indent('a\\r\\n\\r\\nb', '> ', lambda line: line.endswith('\\n'))))\n"
            "print(textwrap.wrap('one two three', width=5), textwrap.shorten('one two three', 9))",
        )

        getters = Path(__file__).with_name("fixtures") / "typed_attrgetter"
        compare("-IB", getters / "fields.py")
        compare("-B", getters / "main.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_attrgetter.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_itemgetter.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_inspect_signature.py")
        static_fixture = Path(__file__).with_name("fixtures") / "boxed_getattr_static.py"
        compare("-IB", static_fixture, native_args=("run", static_fixture, "--runtime-imports", "--quiet"))
        inspect_fixture = Path(__file__).with_name("fixtures") / "boxed_inspect_coroutine.py"
        (root / "view_inspection.py").write_bytes(inspect_fixture.read_bytes())
        inspect_entry = root / "inspect_views.py"
        inspect_entry.write_text("import view_inspection\n")
        compare("-B", inspect_entry, native_args=("run", inspect_entry, "--runtime-imports", "--quiet"))
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_proxy_operators.py")
        compare("-IB", "-c", "from operator import __getitem__; print(__getitem__((11,), 0))")
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
        scoped_import = (
            "import sys\nif sys.argv[1] == 'match':\n"
            "    import os as operating_system\n    from os import sep\n"
            "    print(operating_system.sep == sep)\n"
            "else:\n    try:\n        import _missing_scoped_import_test\n"
            "    except ModuleNotFoundError as error:\n        print(error.name)\n"
        )
        compare("-I", "-c", scoped_import, "match", extra={"IMREFLAGS": "--bogus"})
        compare("-I", "-c", scoped_import, "inverse", extra={"IMREFLAGS": "--bogus"})
        stream_flags = (
            "import sys\n"
            "for f in (sys.stdin, sys.stdout, sys.stderr):\n"
            "    print(f.write_through, f.line_buffering, type(f.buffer).__name__)"
        )
        for option, value in (("-uc", ""), ("-c", "1"), ("-c", "0"), ("-Ec", "1"), ("-Iuc", "0")):
            extra = {"PYTHONUNBUFFERED": value}
            expected = run(sys.executable, option, stream_flags, extra=extra, stdin=b"")
            actual = run(imre, option, stream_flags, extra=extra, stdin=b"")
            assert (actual.returncode, actual.stdout, actual.stderr) == (0, expected.stdout, b"")
        mixed_writes = (
            "import os, sys\n"
            "sys.stdout.write('a'); os.write(1, b'b'); sys.stdout.buffer.write(b'\\xff')\n"
            "sys.stderr.write('c'); os.write(2, b'd'); sys.stderr.buffer.write(b'\\xfe')"
        )
        mixed = run(imre, "-uc", mixed_writes)
        assert (mixed.returncode, mixed.stdout, mixed.stderr) == (0, b"ab\xff", b"cd\xfe")
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
        compare(
            "-B", "-c",
            "import atexit, gc, sys\n"
            "class SelfRemoving:\n"
            "    def __call__(self): atexit.unregister(self)\n"
            "    def __del__(self): atexit.register(print, 'registered during cleanup')\n"
            "atexit.register(SelfRemoving())\n"
            "atexit._run_exitfuncs()\n"
            "assert atexit._ncallbacks() == 0\n"
            "def first(label, *, flag):\n"
            "    import __main__\n"
            "    print(label, flag, __main__.message)\n"
            "atexit.register(first, 'oldest', flag=7)\n"
            "@atexit.register\n"
            "def newest():\n"
            "    print('newest')\n"
            "    atexit.unregister(first)\n"
            "message = 'retained'\n"
            "gc.collect()\n"
            "sys.exit(37)\n",
            status=37,
        )
        compare(
            "-B", "-c",
            "import sys\n"
            "atexit = __import__(sys.argv[1])\n"
            "events = []\n"
            "class Result:\n"
            "    def __del__(self): print('finalized', events)\n"
            "atexit.register(events.append, 'older')\n"
            "atexit.register(Result)\n", "atexit",
        )
        if os.name == "posix":
            immediate_exit = Path(__file__).resolve().parent / "fixtures/os_immediate_exit.py"
            for flags in ((), ("--runtime-imports",)):
                compare("-IB", immediate_exit, status=37,
                        native_args=("run", immediate_exit, "--quiet", "--opt-level", "0", *flags),
                        native_timeout=300)
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

        # Keep lone surrogates distinct from U+FFFD and literal backslashes on the boxed path.
        compare(
            "-c",
            r"""import sys
print(r"\uDCFF", "\\uDCFF", "�", "\ufffd", "\U0001f642", br"\uDCFF", b"\uDCFF")
print(rf"\uDCFF{r'\uDCFF'}")
print(repr("\udcff"), repr(f"{3}\U0000d800"))
scope = {}
exec(r'left = "\ufffd"; right = r"\udcff"', scope)
print(repr(scope["left"]), scope["right"])
sys.path.append("unused")""",
            extra={"PYTHONWARNINGS": "ignore::SyntaxWarning"},
        )
        (root / "surrogate_module.py").write_text(r'value = f"\udfff{3}"')
        compare("-c", "import sys, surrogate_module; sys.path.append('unused'); print(repr(surrogate_module.value))")
        compare("-c", r"""import sys, ast
sys.path.append("unused")
scope = {}
exec(compile(r"x = ('\ud800', '\ufffd' r'\udfff' '\udfff')", "<literal>", "exec"), scope)
print(ascii(scope["x"]))
print(ascii(ast.literal_eval(r"'\ud800' '�' '\udfff'")))
print(ascii(eval(r"'\ud800'")), ascii(eval(compile(r"'\udfff'", "<literal>", "eval"))))
exec(compile(r"def f(x: '\udfff'): pass", "<annotation>", "exec"), scope)
print(ascii(scope["f"].__annotations__))
for expression, expected in [(r"f'\ud800{3}'", "\ud8003"), (r"f'{3}' '\udfff'", "3\udfff")]:
    exec(compile("formatted = " + expression, "<fstring>", "exec"), scope)
    assert scope["formatted"] == expected
sys.displayhook = lambda value: print(ascii(value))
exec(compile(r"'\udfff'", "<interactive>", "single"), scope)
try:
    compile(r"'\t\udfff'", "<docstring>", "exec")
except UnicodeEncodeError as exc:
    print(ascii(exc.args))""")
        compare("-OO", "-c", r"""import sys
sys.path.append("unused")
scope = {}
exec(compile("class C:\n " + r"'\udfff'" + "\n def f(self):\n  " + r"'\ud800'", "<doc>", "exec"), scope)
sys.displayhook = lambda value: print(ascii(value))
exec(compile(r"'\udfff'", "<interactive>", "single"), scope)
print(scope["C"].__doc__, scope["C"].f.__doc__)""")
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

        compare(
            "-B", "-c",
            """import sys
from importlib import import_module
import_members = import_module("import_members")
import_members.__all__ = ["found"]
assert "import_members.found" not in sys.modules
namespace = {"__package__": "import_members"}
exec("from . import *", namespace)
assert namespace["found"] is import_members.found is sys.modules["import_members.found"]
""",
        )

        opcode = run(imre, "-B", "-c", 'op = __import__("".join(("_", "opcode")))\n'
                     'print(op.ENABLE_SPECIALIZATION, op.ENABLE_SPECIALIZATION_FT)')
        assert (opcode.returncode, opcode.stdout, opcode.stderr) == (0, b"0 0\n", b""), opcode

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

            (root / "dangling-link").symlink_to("missing-target")
            compare(
                "-B", "-c",
                "import os\n"
                "from os.path import exists, getatime, getctime, getmtime, lexists\n"
                "assert lexists(path=b'dangling-link') and not exists('dangling-link')\n"
                "s = os.stat('.')\n"
                "assert (getatime('.'), getctime('.'), getmtime('.')) == (s.st_atime, s.st_ctime, s.st_mtime)",
            )

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

        source = root / "boxed_executable.py"
        source.write_text(
            "import subprocess, sys\n"
            "sys.path.append('unused')\n"
            "assert sys._base_executable == sys.executable\n"
            "print(sys.executable)\n"
            "print(sys.prefix)\n"
            "if sys.argv[1:] == ['spawn']:\n"
            "    child = subprocess.run([sys.executable, '-c', "
            "\"import sys; print('child'); sys.exit(47)\"], capture_output=True)\n"
            "    assert (child.stdout, child.stderr, child.returncode) == (b'child\\n', b'', 47)\n"
        )
        image = root / "boxed-executable"
        built = run(imre, "build", "--runtime-imports", source, "-o", image)
        assert built.returncode == 0, built.stderr
        helper_dir = root / "python-helpers"
        helper_dir.mkdir()
        helper = helper_dir / "python3"
        helper.symlink_to(imre)
        for args, extra, expected in (
            ((), {"PATH": ""}, ""),
            ((), {"PATH": str(helper_dir)}, str(helper)),
            (("spawn",), {"IMRE_PYTHON": str(helper.relative_to(root))}, str(helper_dir.resolve() / "python3")),
        ):
            result = run(image, *args, extra=extra)
            assert result.returncode == 0, result.stderr
            assert result.stdout.decode().splitlines() == [expected, str(root.resolve())]

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
    for template in ("{absent}", "{name:{width}}", "{it's missing}"):
        try:
            template.format(**fields)
        except KeyError as err:
            print(str(err), err.args)
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

        token_limit = Path(__file__).with_name("fixtures") / "json_integer_token_limit.py"
        for limit in ("640", "0"):
            digit_env = {"PYTHONINTMAXSTRDIGITS": limit}
            compare("-B", token_limit, extra=digit_env)
            compare("-B", token_limit, extra=digit_env, native_args=("run", token_limit, "--runtime-imports", "--quiet"))

        (helpers / "uuid").mkdir()
        uuid_import = f"import sys\nsys.path.insert(0, {str(helpers)!r})\nimport uuid\n"
        compare("-IB", "-c", uuid_import + "print(hasattr(uuid, '__path__'), str(uuid.UUID(int=0)))")
        (helpers / "uuid.py").write_text("marker = 'explicit search path'\n")
        compare("-IB", "-c", uuid_import + "print(uuid.marker)")

        (helpers / "pprint.py").write_text("def incomplete(:\n")
        compare("-IB", "-c", f"import sys\nsys.path.insert(0, {str(helpers)!r})\nprint(__import__('sys').__name__)")
        (helpers / "pprint.py").unlink()

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

        compare(
            "-IB", "-c",
            "import re, sysconfig\n"
            "for cls, pattern in [(re.error, '('), (re.PatternError, b'[')]:\n"
            "    try: re.compile(pattern)\n"
            "    except cls as exc: print(type(exc) is re.PatternError, exc.pattern == pattern, exc.pos)\n"
            "print(re.error is re.PatternError, issubclass(re.error, Exception))",
        )

        resource_package = installed / "pkgutil_resource"
        resource_package.mkdir()
        (resource_package / "__init__.py").write_text("print('resource package initialized')\n")
        (resource_package / "payload.bin").write_bytes(b"\x00\xff\r\n")
        fixtures = Path(__file__).with_name("fixtures")
        compare("-B", "-c", "from typing import Protocol as P; print(P.__name__)")
        compare("-B", fixtures / "typing_protocol_import.py")
        compare(
            "-IB", "-c",
            "from datetime import datetime, timezone\n"
            "d = datetime(2000, 1, 2, 3, tzinfo=timezone.utc)\n"
            "print(d.astimezone(), d.astimezone(tz=None))",
            extra={"TZ": "EST5"},
        )
        compare("-IB", fixtures / "boxed_pickle_writer.py")
        compare("-IB", "-c", "import io, logging; print(repr(logging.StreamHandler(io.StringIO())))")
        compare("-IB", "-c", "import io; b = io.BytesIO(b'abc'); print(bytes(b.getbuffer()))")
        compare(
            "-IB", "-c",
            "import io; b = io.BytesIO(b'abc'); get = b.getbuffer; v = get(); v[1] = 90; print(b.getvalue())",
        )
        compare(
            "-B", "-c",
            "import typing as t\ndef build():\n    class P(t.Protocol): pass\n"
            "    return P._is_protocol\nprint(build())",
        )
        final_fixture = fixtures / "boxed_typing_final.py"
        compare("-IB", final_fixture, native_args=("run", final_fixture, "--runtime-imports", "--quiet"))
        io_registration = fixtures / "io_abc_registration.py"
        compare("-IB", io_registration, native_args=("run", io_registration, "--quiet"))
        override_fixture = fixtures / "boxed_typing_override.py"
        compare("-IB", override_fixture, native_args=("run", override_fixture, "--runtime-imports", "--quiet"))
        reader_fixture = fixtures / "io_buffered_reader_type.py"
        compare("-IB", reader_fixture, native_args=("run", reader_fixture, "--quiet"))
        compare(
            "-IB", "-c",
            "import codecs, io; choose = codecs.getwriter; sink = io.BytesIO(); "
            "choose('cp1251')(sink).write('до'); print(sink.getvalue())",
        )
        compare(
            "-IB", "-c",
            "import codecs\nclass Sink:\n    def write(self, data: bytes) -> None: print(data)\n"
            "codecs.getwriter('cp1251')(Sink()).write('до')",
        )
        reinit_writer = fixtures / "io_buffered_writer_reinitialize.py"
        reinit_path = root / "reinitialized-writer.bin"
        compare("-IB", reinit_writer, reinit_path, native_args=("run", reinit_writer, "--quiet", "--", reinit_path))
        callback_writer = fixtures / "io_buffered_writer_callbacks.py"
        compare("-IB", callback_writer, native_args=("run", callback_writer, "--quiet"))
        shutdown_writer = fixtures / "io_buffered_writer_shutdown.py"
        sink = root / "callback-shutdown.bin"
        command = ("run", shutdown_writer, "--quiet", "--", sink)
        for mode in ("close", "temporary-close", "file-finalize", "callback-finalize"):
            compare("-IB", shutdown_writer, sink, mode, native_args=(*command, mode))
            assert sink.read_bytes() == b"abc"
        pending = run(imre, *command, "pending")
        assert pending.returncode == 1 and b"unflushed bytes at shutdown" in pending.stderr, pending
        assert sink.read_bytes() == b""
        random_fixture = fixtures / "io_buffered_random_type.py"
        compare("-IB", random_fixture, native_args=("run", random_fixture, "--quiet"))
        memory_writer = fixtures / "io_buffered_writer_bytesio.py"
        compare("-IB", memory_writer, native_args=("run", memory_writer, "--quiet"))
        pair_fixture = fixtures / "io_buffered_rw_pair.py"
        compare("-IB", pair_fixture, native_args=("run", pair_fixture, "--quiet"))
        writer_fixture = fixtures / "io_buffered_writer_type.py"
        compare("-IB", writer_fixture, native_args=("run", writer_fixture, "--quiet"))
        io_fixture = fixtures / "boxed_io_indirect_import.py"
        compare("-IB", io_fixture, native_args=("run", io_fixture, "--runtime-imports", "--quiet"))
        coroutine_fixture = fixtures / "boxed_prelinked_coroutine.py"
        compare("-IB", coroutine_fixture, native_args=("run", coroutine_fixture, "--runtime-imports", "--quiet"))
        builtins_fixture = fixtures / "boxed_builtins_module.py"
        compare("-IB", builtins_fixture, native_args=("run", builtins_fixture, "--runtime-imports", "--quiet"))
        compare(
            "-I", "-c",
            "import atexit, threading\n"
            "seen = []\n"
            "def register(value):\n"
            "    def callback(*, suffix): seen.append(value + suffix)\n"
            "    threading._register_atexit(callback, suffix='!')\n"
            "register('first')\n"
            "threading._register_atexit(seen.append, 'last')\n"
            "def check():\n"
            "    main = threading.main_thread()\n"
            "    print(seen, main.is_alive(), 'stopped' in repr(main))\n"
            "atexit.register(check)\n"
            "threading._shutdown()\n"
            "threading._shutdown()",
        )
        for name in (
            "boxed_threading_deprecated_accessors.py",
            "boxed_lowlevel_thread.py", "boxed_lowlevel_thread_roots.py",
            "boxed_thread_dangling.py", "boxed_threading_rlock.py", "boxed_indirect_threading_rlock.py",
            "boxed_threading_condition_event.py", "boxed_thread_captured_lock.py",
            "boxed_thread_bound_lock.py",
            "boxed_thread_captured_acquire.py",
            "boxed_thread_hash_update.py",
        ):
            thread_fixture = fixtures / name
            compare("-IB", thread_fixture, native_args=("run", thread_fixture, "--runtime-imports", "--quiet"))
        metaclass_new = fixtures / "ordinary_metaclass_new.py"
        compare("-IB", metaclass_new, native_args=("run", metaclass_new, "--runtime-imports", "--quiet"))
        class_locals = fixtures / "boxed_class_locals.py"
        compare("-IB", class_locals, native_args=("run", class_locals, "--runtime-imports", "--quiet"))
        generic = fixtures / "module_generic_function.py"
        compare("-IB", generic, native_args=("run", generic, "--quiet"))
        compare("-IB", generic, native_args=("run", generic, "--runtime-imports", "--quiet"))
        compare("-IB", "-c", "import typing; from typing import Text; print(Text is typing.Text is str)")
        reveal = fixtures / "typing_reveal_type.py"
        for command in ((sys.executable, "-IB", reveal),
                        (imre, "run", reveal, "--quiet"),
                        (imre, "run", reveal, "--runtime-imports", "--quiet")):
            result = run(*command)
            assert (result.returncode, result.stdout, result.stderr) == (
                0, b"", b"Runtime type is 'Marker'\nRuntime type is 'type'\n",
            ), result
        introspection = fixtures / "typing_introspection_functions.py"
        compare("-IB", introspection, native_args=("run", introspection, "--quiet"))
        compare("-IB", introspection, native_args=("run", introspection, "--runtime-imports", "--quiet"))
        hints_fixture = fixtures / "boxed_typing_get_type_hints.py"
        compare("-IB", hints_fixture, native_args=("run", hints_fixture, "--runtime-imports", "--quiet"))
        if sys.platform == "linux":
            exe = root / "marshal-allocation"
            built = run(imre, "build", fixtures / "boxed_marshal_allocation.py",
                        "--quiet", "--no-cache", "--runtime-imports", "-o", exe)
            assert built.returncode == 0, built.stderr
            check_marshal_allocation(exe)
        if sys.platform == "linux":
            import resource

            source = root / "nested.py"
            source.write_text("".join(" " * i + "if True:\n" for i in range(96)) + " " * 96 + "result = 7\n")
            executable = root / "source-branches"
            built = run(imre, "build", fixtures / "boxed_source_lower_stack.py", "--runtime-imports", "-o", executable)
            assert built.returncode == 0, built.stderr

            def small_stack():
                resource.setrlimit(resource.RLIMIT_STACK, (96 * 1024, 96 * 1024))

            result = subprocess.run([executable, source], capture_output=True, preexec_fn=small_stack)
            assert (result.returncode, result.stdout) == (0, b"7\n"), result.stderr
        compare(
            "-IB", "-c",
            "decoder = __import__(''.join(('co', 'decs'))).IncrementalDecoder\n"
            "print(decoder.__module__, decoder('replace').errors, decoder().getstate())",
        )
        compare(
            "-IB", "-c",
            "import sys\n"
            "print(2 | 4, 'typing' in sys.modules)\n"
            "union = int\n"
            "union |= str\n"
            "union |= 'C'\n"
            "reference = union.__args__[-1]\n"
            "print(type(reference).__name__, reference.__forward_arg__)",
        )
        split_entry = root / "split_identity.py"
        split_entry.write_text(
            "from urllib.parse import urlsplit\n"
            "first = urlsplit('http://host/a')\n"
            "print(first is urlsplit('http://host/a'), first is urlsplit(b'http://host/a'))\n"
        )
        compare("-IB", split_entry, native_args=("run", split_entry, "--quiet"))
        regex_entry = root / "regex_entry.py"
        for program in (
            "import re as rx\nprint(rx.split(b'(:+)', b'a::b:c', maxsplit=1))",
            "from re import subn as replace\nprint(replace('x*', '!', 'abxd', count=3))",
            "import re\npattern = re.compile(b'(:*)')\nprint(pattern.split(b':a:b', 2))",
            "from re import compile as make\nreplace = make('x*').subn\nprint(replace('!', 'abxd', 3))",
        ):
            regex_entry.write_text(program)
            compare("-IB", regex_entry, native_args=("run", regex_entry, "--quiet"))
        radix_entry = root / "radix_entry.py"
        for program in (
            "class Offset:\n    def __index__(self): return -(1 << 100)\nprint(hex(Offset()))",
            "formatter = hex\nprint(formatter(31))",
            "formatter = oct\nprint(formatter(0), oct(-(1 << 150)), oct(True))",
            "import builtins\nfrom builtins import oct as formatter\n"
            "class Number:\n    def __index__(self): return 0o624\n"
            "print(formatter is builtins.oct, formatter(Number()))",
            "import builtins\nprint(builtins.hex(31))",
            "from builtins import hex as formatter\nprint(formatter(31))",
            "import warnings\nclass Flag:\n    def __index__(self): return True\n"
            "warnings.simplefilter('error', DeprecationWarning)\n"
            "try: hex(Flag())\nexcept DeprecationWarning as warning: print(warning)",
        ):
            radix_entry.write_text(program)
            compare("-IB", radix_entry, native_args=("run", radix_entry, "--quiet"))
        monitoring_entry = root / "monitoring_entry.py"
        (root / "monitoring_helper.py").write_text(
            "from sys import monitoring\ndef line_mask():\n    return monitoring.events.LINE\n"
        )
        for program in (
            "from monitoring_helper import line_mask\nprint(line_mask())\n",
            "import sys\nprint(sys.monitoring.events.LINE)\n",
            "from sys import monitoring\nprint(monitoring.events.LINE)\n",
        ):
            monitoring_entry.write_text(program)
            compare("-EB", monitoring_entry, native_args=("run", monitoring_entry, "--quiet"))
        compare("-IB", fixtures / "boxed_unittest_named_loader.py")
        compare("-IB", fixtures / "boxed_unittest_patch.py")
        call_args = fixtures / "unittest_mock_call_args.py"
        compare("-IB", call_args, native_args=("run", call_args, "--quiet"))
        boxed_call_args = fixtures / "boxed_unittest_call_args.py"
        compare("-IB", boxed_call_args, native_args=("run", boxed_call_args, "--runtime-imports", "--quiet"))
        mock_compile_flags = {"IMREFLAGS": "--opt-level 0"}
        compare(
            "-B", "-c", "from unittest.mock import Mock\nm = Mock(['close'])\nm.close()\nassert not hasattr(m, 'read')\n",
            extra=mock_compile_flags, native_timeout=300,
        )
        open_fixture = fixtures / "boxed_unittest_mock_open.py"
        for import_flags in ((), ("--runtime-imports",)):
            compare(
                "-IB", open_fixture, extra=mock_compile_flags,
                native_args=("run", open_fixture, *import_flags, "--quiet"),
                native_timeout=300,
            )
        for name in ("boxed_unittest_mock_any.py", "boxed_unittest_mock_side_effect.py", "boxed_unittest_mock_call.py"):
            fixture = fixtures / name
            compare("-IB", fixture, native_args=("run", fixture, "--quiet"))
            compare("-IB", fixture, native_args=("run", fixture, "--runtime-imports", "--quiet"))
        for fixture in (
            "boxed_ast.py",
            "boxed_ast_visitors.py",
            "boxed_ast_literal_eval.py",
            "boxed_code_objects.py",
            "boxed_inspect_getmodule.py",
            "boxed_inspect_unwrap.py",
            "boxed_inspect_isroutine.py",
            "boxed_numeric_repr.py",
            "boxed_recursive_repr.py",
            "boxed_bound_method_repr.py",
            "boxed_simple_namespace.py",
            "boxed_namedtuple_repr.py",
            "boxed_namedtuple_factory.py",
            "boxed_slice_deep_repr.py",
            "boxed_slice_repr_cycles.py",
            "boxed_source_exec.py",
            "boxed_source_except.py",
            "boxed_interactive.py",
            "boxed_relative_package_imports.py",
            "boxed_sys_getframe.py",
            "boxed_sys_getsizeof.py",
            "boxed_sys_audit.py",
            "boxed_sys_audit_recursion.py",
            "boxed_sys_recursion_limit.py",
            "boxed_sys_intern.py",
            "boxed_sys_byteorder.py",
            "boxed_sys_hash_info.py",
            "boxed_sys_thread_info.py",
            "boxed_sys_flags.py",
            "boxed_sys_jit.py",
            "boxed_sys_monitoring_events.py",
            "boxed_bytes_add.py",
            "boxed_binary_iterators.py",
            "boxed_array.py",
            "boxed_bytearray_memoryview.py",
            "boxed_bytes_io.py",
            "boxed_io_newline_decoder.py",
            "boxed_io_utf16.py",
            "boxed_text_io_wrapper.py",
            "boxed_platform_version.py",
            "boxed_int_to_bytes.py",
            "boxed_int_rational_fields.py",
            "boxed_int_from_bytes.py",
            "boxed_divmod_slots.py",
            "boxed_complex.py",
            "boxed_float.py",
            "boxed_round.py",
            "boxed_math_rounding.py",
            "boxed_math_gcd.py",
            "boxed_float_arithmetic.py",
            "boxed_bytes_fields.py",
            "boxed_binary_subclass_lengths.py",
            "boxed_binary_strip.py",
            "boxed_class_union.py",
            "boxed_bytes_octal.py",
            "boxed_contextvars.py",
            "boxed_temporary_file.py",
            "boxed_spooled_temporary_file.py",
            "boxed_named_temporary_file.py",
            "boxed_sys_finalizing.py",
            "boxed_named_temporary_writer.py",
            "boxed_temporary_directory.py",
            "boxed_tempfile_default_directory.py",
            "boxed_tempfile_mktemp.py",
            "boxed_tempfile_wrapper.py",
            "boxed_random.py",
            "boxed_secrets.py",
            "boxed_pkgutil.py",
            "boxed_lru_cache_equality.py",
            "boxed_lru_cache_metadata.py",
            "boxed_functools_cache.py",
            "boxed_functools_reduce.py",
            "boxed_functools_singledispatch.py",
            "boxed_functools_partialmethod.py",
            "boxed_contextmanager.py",
            "boxed_total_ordering.py",
            "boxed_cmp_to_key.py",
            "boxed_lru_insert_eviction.py",
            "boxed_lru_insert_same.py",
            "boxed_annotationlib_refs.py",
            "boxed_class_annotation_sites.py",
            "boxed_future_module_annotations.py",
            "boxed_deferred_module_annotations.py",
            "boxed_typing_no_type_check.py",
            "boxed_typing_no_type_check_decorator.py",
            "boxed_typing_assert_type.py",
            "boxed_typing_type_predicates.py",
            "boxed_typing_bytestring.py",
            "boxed_typing_new_type.py",
            "boxed_typing_typeddict.py",
            "boxed_typing_dataclass_transform.py",
            "boxed_typing_optional.py",
            "boxed_typing_literal.py",
            "boxed_typing_protocol.py",
            "boxed_typing_typevar_members.py",
            "boxed_typing_parameter_collector.py",
            "boxed_typing_alias_namespaces.py",
            "boxed_abc_callable_alias.py",
            "boxed_typing_variadic.py",
            "boxed_typing_tuple.py",
            "boxed_typing_async_collection_aliases.py",
            "boxed_typing_abc_aliases.py",
            "boxed_typing_alias_defaults.py",
            "boxed_typing_type.py",
            "boxed_type_alias_statements.py",
            "boxed_typing_mapping.py",
            "boxed_typing_concrete_collections.py",
            "boxed_typing_iterable.py",
            "boxed_typing_regex_aliases.py",
            "boxed_pprint.py",
            "boxed_formatter.py",
            "boxed_template_literals.py",
            "boxed_re_ascii.py",
            "boxed_re_verbose.py",
            "boxed_re_multiline_dollar.py",
            "boxed_re_positive_lookahead.py",
            "boxed_re_lookahead.py",
            "boxed_re_split.py",
            "boxed_re_match_subscript.py",
            "boxed_re_pattern_cache.py",
            "boxed_re_sub.py",
            "boxed_re_findall_empty.py",
            "boxed_memoryview_context.py",
            "boxed_re_scanner.py",
            "boxed_re_escape.py",
            "boxed_datetime.py",
            "boxed_date_comparison.py",
            "boxed_timedelta_multiply.py",
            "boxed_timedelta_scale.py",
            "boxed_timedelta_floordiv.py",
            "boxed_timedelta_add_sub.py",
            "boxed_dataclasses.py",
            "boxed_mapping_proxy_type.py",
            "boxed_types_new_class.py",
            "boxed_make_dataclass.py",
            "boxed_dataclass_slots.py",
            "boxed_dataclass_init_annotations.py",
            "boxed_dataclass_classvars.py",
            "boxed_email_dates.py",
            "boxed_email_addresses.py",
            "boxed_http_message.py",
            "boxed_time.py",
            "boxed_time_timezone.py",
            "boxed_time_strftime.py",
            "boxed_codecs.py",
            "boxed_codec_registry.py",
            "boxed_wide_decode.py",
            "boxed_buffered_partial_write.py",
            "boxed_io_exports.py",
            "boxed_io_unsupported_operation.py",
            "boxed_io_reimport.py",
            "boxed_fileio_opener.py",
            "boxed_open_opener.py",
            "boxed_io_text_encoding.py",
            "boxed_locale.py",
            "boxed_locale_error.py",
            "boxed_io_protocols.py",
            "boxed_io_abc_registration.py",
            "boxed_importlib_abc.py",
            "boxed_logging_handlers.py",
            "boxed_logging_handler_list.py",
            "boxed_logging_handler_filters.py",
            "boxed_logging_rlock.py",
            "boxed_selector_wide_fds.py",
            "boxed_select_selector.py",
            "boxed_pipe_flags.py",
            "boxed_os_environb.py",
            "boxed_os_python_fspath.py",
            "boxed_os_file_descriptors.py",
            "boxed_os_fdopen.py",
            "boxed_binary_bytes_paths.py",
            "boxed_buffered_readlines.py",
            "boxed_os_eventfd.py",
            "boxed_os_descriptor_io.py",
            "boxed_os_ftruncate.py",
            "boxed_os_pread.py",
            "boxed_os_readinto.py",
            "boxed_os_waitpid.py",
            "boxed_os_process_signal.py",
            "boxed_posix_session.py",
            "boxed_os_stat.py",
            "boxed_os_terminal_size.py",
            "boxed_os_fchmod.py",
            "boxed_os_makedirs.py",
            "boxed_os_walk.py",
            "boxed_os_fwalk.py",
            "boxed_select.py",
            "boxed_urlsplit.py",
            "boxed_urllib_request.py",
            "boxed_urllib_file.py",
            "boxed_urllib_cleanup.py",
            "boxed_urllib_retrieve.py",
            "boxed_socket_timeout.py",
            "boxed_socketserver_source.py",
            "boxed_urllib_data.py",
            "boxed_urllib_http_status.py",
            "boxed_urllib_ftp.py",
            "boxed_urllib_http.py",
            "boxed_http_transport.py",
            "boxed_message_parameters.py",
            "boxed_unicodedata.py",
            "boxed_descriptors.py",
            "boxed_exception_repr.py",
            "boxed_exception_notes.py",
            "boxed_warning_categories.py",
            "boxed_warnings.py",
            "boxed_oserror.py",
            "boxed_type_identity.py",
            "boxed_type_namespace.py",
            "boxed_abc_namespace.py",
            "boxed_singleton_types.py",
            "boxed_singleton_subclass.py",
            "boxed_function_closure_cells.py",
            "boxed_function_closure_edges.py",
            "boxed_function_annotation_closure.py",
            "boxed_functiontype.py",
            "boxed_function_call.py",
            "boxed_any_generator.py",
            "boxed_global.py",
            "boxed_configparser.py",
            "boxed_argparse.py",
            "boxed_getopt.py",
            "boxed_logging_config.py",
            "boxed_logging.py",
            "boxed_logging_manager.py",
            "boxed_logging_formatter.py",
            "boxed_logging_file.py",
            "boxed_logging_adapter.py",
            "boxed_logging_filter.py",
            "boxed_logging_file_error.py",
            "boxed_class_attribute.py",
            "boxed_class_locals.py",
            "boxed_missing_attribute_owners.py",
            "boxed_request_strings.py",
            "boxed_str_maketrans.py",
            "boxed_map_capwords.py",
            "boxed_module_globals.py",
            "boxed_dir.py",
            "boxed_module_dir.py",
            "boxed_unittest_assert_almost_equal.py",
            "boxed_unittest_assert_count_equal.py",
            "boxed_unittest_assert_set_equal.py",
            "boxed_unittest_membership.py",
            "boxed_unittest_endswith.py",
            "boxed_unittest_sequence_equal.py",
            "boxed_unittest_instance.py",
            "boxed_unittest_subclass.py",
            "boxed_unittest_not_has_attr.py",
            "boxed_unittest_not_equal.py",
            "boxed_unittest_ordering.py",
            "boxed_unittest_assert_raises.py",
            "boxed_unittest_assert_warns.py",
            "boxed_unittest_assert_logs.py",
            "boxed_unittest_assert_regex.py",
            "boxed_unittest_identity.py",
            "boxed_unittest_skip.py",
            "boxed_unittest_seal.py",
            "boxed_frozenset.py",
            "boxed_attribute_deletion.py",
            "boxed_recursion.py",
            "boxed_function_defaults.py",
            "boxed_reprlib.py",
            "boxed_asyncio_policy.py",
            "boxed_multiprocessing_context.py",
            "boxed_textwrap.py",
            "boxed_set_ordering.py",
            "boxed_dict_view_equality.py",
            "boxed_set_issubset.py",
            "boxed_set_operator_descriptors.py",
            "boxed_set_union.py",
            "boxed_set_difference_update.py",
            "boxed_set_difference.py",
            "boxed_set_intersection.py",
            "boxed_set_pop.py",
            "boxed_set_remove.py",
            "boxed_weakref.py",
            "boxed_public_weakref.py",
            "boxed_weakref_unraisable.py",
            "boxed_gc_collect.py",
            "boxed_gc_controls.py",
            "boxed_gc_finalizers.py",
            "boxed_weakref_subclass.py",
            "boxed_proxy_truth.py",
            "boxed_setattr.py",
            "boxed_super.py",
            "boxed_property_setter.py",
            "boxed_cached_property.py",
            "boxed_staticmethod.py",
            "boxed_classmethod_attributes.py",
            "boxed_item_assignment.py",
            "boxed_bytecache.py",
            "boxed_fnmatch.py",
            "boxed_unittest_cleanup.py",
            "boxed_unittest_result_registry.py",
            "boxed_unittest_get_description.py",
            "boxed_unittest_default_result.py",
            "boxed_unittest_runner_warnings.py",
            "boxed_unittest_runner_durations.py",
            "boxed_unittest_class_cleanup.py",
            "boxed_pathlib.py",
            "boxed_posixpath.py",
            "boxed_linecache.py",
            "boxed_stat.py",
            "boxed_signal.py",
            "boxed_itertools_count.py",
            "boxed_enum_descriptors.py",
            "boxed_enum_auto.py",
            "boxed_global_enum.py",
            "boxed_float_enum.py",
            "boxed_print_streams.py",
            "boxed_redirect_streams.py",
            "boxed_with_lookup.py",
            "boxed_itertools_repeat.py",
            "boxed_itertools_type_namespaces.py",
            "boxed_itertools_cursor_repr.py",
            "boxed_itertools_chain.py",
            "boxed_itertools_cycle.py",
            "boxed_itertools_batched.py",
            "boxed_itertools_tee.py",
            "boxed_itertools_combinations.py",
            "boxed_itertools_permutations.py",
            "boxed_itertools_product.py",
            "boxed_itertools_groupby.py",
            "boxed_itertools_filterfalse.py",
            "boxed_object_new.py",
            "boxed_module_type.py",
            "boxed_source_module_getattr.py",
            "boxed_thread_local.py",
            "boxed_thread_local_subclass.py",
            "boxed_threading_lock.py",
            "boxed_import_depth.py",
            "boxed_dict_subclasses.py",
            "boxed_dict_union.py",
            "boxed_abc_metaclass.py",
            "boxed_bless_loader.py",
            "boxed_mapping_abc.py",
            "boxed_user_containers.py",
            "boxed_dict_pop.py",
            "boxed_recursive_equality.py",
            "boxed_thread_rlock.py",
            "boxed_selector_modify_data.py",
            "boxed_structural_hooks.py",
            "boxed_list_pop.py",
            "boxed_sequence_repeat.py",
            "boxed_sequence_order.py",
            "boxed_tuple_index.py",
            "boxed_json_containers.py",
            "boxed_json_scanner.py",
            "boxed_json_decoder.py",
            "boxed_json_module_doc.py",
            "boxed_json_encoder.py",
            "boxed_json_encoder_accelerator.py",
            "boxed_json_quote.py",
            "boxed_deque.py",
            "boxed_deque_iterator_reduction.py",
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
            "boxed_source_coroutine.py",
            "boxed_source_async_with.py",
            "boxed_with_control.py",
            "boxed_short_circuit.py",
            "boxed_doctest_suite.py",
            "boxed_slice.py",
            "boxed_binascii.py",
            "boxed_zlib.py",
            "boxed_hashlib_names.py",
            "bytes/hashlib_md5_provider.py",
            "bytes/hashlib_sha3_provider.py",
            "bytes/hashlib_kdf.py",
            "boxed_hashlib_file_digest.py",
            "boxed_standard_base64.py",
            "boxed_csv.py",
            "boxed_bytes_count.py",
            "boxed_bytes_separators.py",
            "boxed_binary_splitlines.py",
            "boxed_bytes_join.py",
            "boxed_bytes_fromhex.py",
            "boxed_binary_endswith.py",
            "boxed_struct.py",
            "boxed_pickle_writer.py",
            "boxed_reversed_list.py",
            "boxed_reversed_range.py",
            "boxed_heapq.py",
            "boxed_queue.py",
            "boxed_shutil.py",
            "boxed_difflib.py",
            "boxed_colorize.py",
            "boxed_string_new.py",
            "boxed_string_repeat.py",
            "boxed_string_surrogates.py",
            "boxed_string_strip.py",
            "boxed_string_expandtabs.py",
        ):
            if fixture in ("boxed_stat.py", "boxed_signal.py") and sys.platform not in ("linux", "darwin"):
                continue
            plugin.write_bytes((fixtures / fixture).read_bytes())
            if fixture == "boxed_slice_deep_repr.py":
                check_slice_stack(imre, imports, root, env)
            else:
                compare("-IB", "-c", imports + "arithmetic.current()")
            if fixture == "boxed_json_module_doc.py":
                compare("-IOOB", "-c", imports + "arithmetic.current()")
            if fixture == "boxed_interactive.py":
                for stdio in ("utf-8:strict", "utf-8:surrogateescape", "ascii:surrogateescape",
                              "cp1251:surrogateescape", "utf-8-sig:strict", "ascii:replace", "ascii:ignore"):
                    compare("-B", "-c", imports + "arithmetic.original_stream()",
                            extra={"PYTHONIOENCODING": stdio})
                for hook in (False, True):
                    compare("-B", "-c", imports + f"arithmetic.first_write({hook})",
                            extra={"PYTHONIOENCODING": "utf-8-sig:strict"})
            if fixture == "boxed_recursive_repr.py":
                for limit in (640, 4300, 0):
                    compare(
                        "-B", "-c", imports + f"arithmetic.integer_digit_limit({limit})",
                        extra={"PYTHONINTMAXSTRDIGITS": str(limit)},
                    )
        if os.name == "posix":
            check_boxed_unittest_main_reporting(imre, root, run)
            check_unittest_main_names(imre, root, run)
            check_unittest_file_discovery(imre, root, run)
            plugin.write_bytes((fixtures / "boxed_subprocess_fileio.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_exports.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_interruptible_read.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_pass_fds.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_buffering.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_text.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_chdir.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_capabilities.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_mimetypes.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_urandom.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            os.symlink(b"unresolved-\xff", root / "readlink-source")
            plugin.write_bytes((fixtures / "boxed_os_readlink.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_symlink.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_rename.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_scandir.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
        if sys.platform == "linux":
            plugin.write_bytes((fixtures / "boxed_posix_fadvise.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_fallocate.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_sched_param.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_sched_getscheduler.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_fcntl.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_rtld_flags.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_sysconf.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_resource.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-IB", "-c", imports + "arithmetic.core_limits_child()")
            compare("-IB", "-c", imports + "arithmetic.nofile_limits_child()")
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
        # A nested declaration is invalid even if the surrounding function is never called.
        source = root / "generic-nonlocal.py"
        source.write_text("def factory():\n    class Box[T]:\n        def f(self):\n            nonlocal T\n")
        invalid = run(imre, "build", source, "--runtime-imports", "-o", root / "invalid-nonlocal")
        assert invalid.returncode and b"SyntaxError: nonlocal binding not allowed for type parameter 'T'" in invalid.stderr, invalid

        for fixture in ("boxed_typing_generics.py", "boxed_typing_list.py",
                        "boxed_typing_set_aliases.py", "boxed_typing_concatenate.py"):
            plugin.write_bytes((fixtures / fixture).read_bytes())
            compare("-IB", "-c", imports)
        if sys.platform.startswith("linux"):
            import resource

            source = root / "nested-functions.py"
            source.write_text("".join(" " * i + "def f():\n" for i in range(64)) + " " * 64 + "pass\n")
            executable = root / "walk-statements"
            fixture = fixtures / "boxed_ast_statement_stack.py"
            built = run(imre, "build", fixture, "--runtime-imports", "-o", executable)
            assert built.returncode == 0, built.stderr

            def small_stack():
                resource.setrlimit(resource.RLIMIT_STACK, (96 * 1024, 96 * 1024))

            result = subprocess.run([executable, source], capture_output=True, preexec_fn=small_stack)
            assert (result.returncode, result.stdout) == (0, b"130\n"), result.stderr

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
        compare(
            "-IB", "-c",
            "with open('written.bin', 'wb') as f:\n    f.writelines([b'a', b'b'])\n"
            "with open('written.bin', 'rb') as f:\n    print(f.read())",
        )
        plugin.write_bytes((Path(__file__).with_name("fixtures") / "boxed_io.py").read_bytes())
        compare("-IB", "-c", imports + "arithmetic.exercise('chunks.bin')")
        compare(
            "-IB", "-c",
            "with open('chunks.bin', 'rb') as stream:\n    print(stream.name, stream.mode)\n"
            "print(stream.name, stream.mode)",
        )
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
            "class Input(bytearray): pass\nclass Suffix(bytes): pass\n"
            "source = Input(b'ab')\nh = hashlib.sha256(string=source)\n"
            "source[0] = 99\nclone = h.copy()\nh.update(Suffix(b'c'))\n"
            "print(h.hexdigest(), clone.hexdigest(), list(h.digest()))\n"
            "for args, kw, message in [((b'a',), {'data': b'b', '_': 1}, 'given by name'), "
            "((), {'data': b'a', 'string': b'b', '_': 1}, 'unexpected keyword argument'), "
            "((b'a',), {'string': b'b'}, 'mutually exclusive')]:\n"
            "    try:\n        hashlib.sha1(*args, **kw)\n"
            "    except TypeError as exc:\n        print(message in str(exc))\n"
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
        digits = fixtures / "json_integer_digit_limit.py"
        for limit in ("640", "0"):
            digit_env = {"PYTHONINTMAXSTRDIGITS": limit}
            compare("-B", digits, extra=digit_env)
            compare("-B", digits, extra=digit_env, native_args=("run", digits, "--runtime-imports", "--quiet"))

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
from io import StringIO
stream = StringIO()
print(json.dump(fp=stream, obj={"a": [1, 2]}), stream.getvalue())
stream = StringIO()
try:
    json.dump([1, object()], stream)
except TypeError:
    print(stream.getvalue())
class StreamItems(list):
    def __iter__(self):
        yield 1
        raise LookupError("iterator stopped")
stream = StringIO()
try:
    json.dump(StreamItems([1]), stream)
except LookupError as err:
    print(str(err), stream.getvalue())
print(json.dumps({"z": Extra(), "a": json.loads("1.25")}, **options))
print(repr(json.dumps({"items": [1, {}]}, indent=2)))
print(repr(json.dumps([Extra()], indent="--", **options)))
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
