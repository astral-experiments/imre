"""Compare the script and command-string entry points with Python's CLI."""

import argparse
import errno
import gzip
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

from fixture_entry import imported_fixture


def check_registered_entry_operand(imre, root, run):
    entry = root / "checked_operand.py"
    functions = (
        "def publish(a: int, b: int, send):\n    send(a + b)\n"
        "def report(value):\n    print(value)\n"
    )
    entry.write_text(
        "def model() -> int:\n"
        "    items: list[int] = [3]\n"
        "    items.append(True)\n"
        "    return items[0]\n"
        "print('reached')\nprint(model())\n"
    )
    for argv in ((entry,), ("-m", entry.stem)):
        result = run(imre, *argv)
        assert result.returncode and not result.stdout, result
        assert b"checked_operand.py:3:18: Imre: expected int, got bool" in result.stderr, result

    saved = (
        "def model(value: int) -> int:\n"
        "    items = [3]\n    items.append(True)\n    return items[0] + value\n"
        "def stop() -> int:\n    print('stopped')\n    raise SystemExit(13)\n"
        "saved = stop\ndef stop() -> int:\n    return 7\n"
    )
    entry.write_text(saved + "print(model(saved()))\n")
    result = run(imre, "-m", entry.stem)
    assert (result.returncode, result.stdout, result.stderr) == (13, b"stopped\n", b""), result
    entry.write_text(saved + "print(model(stop()))\n")
    result = run(imre, "-m", entry.stem)
    assert result.returncode and not result.stdout, result
    assert b"checked_operand.py:3:18: Imre: expected int, got bool" in result.stderr, result

    entry.write_text(
        "def show(): print(1)\n"
        "def model():\n    items: list[int] = []\n    items.append(True)\n"
        "show()\nmodel()\n"
    )
    result = run(imre, "-m", entry.stem)
    assert result.returncode and not result.stdout, result
    assert b"checked_operand.py:4:18: Imre: expected int, got bool" in result.stderr, result

    setup = "send = {'send': report}[input()]\npublish(3, 5, send)\n"
    before = functions + setup + "values: list[int] = [3]\noffset = 17\n"
    for use in ("values.append(True)", "result = values.append(True)", "print(values.append(True))"):
        entry.write_text(before + use + "\n")
        result = run(imre, "-m", "checked_operand", stdin=b"send\n")
        column = use.index("True") + 1
        assert result.returncode and result.stdout == b"", result
        assert f"checked_operand.py:9:{column}: Imre: expected int, got bool".encode() in result.stderr, result
    entry.write_text(before + "if False:\n    values.append(True)\nprint(values)\n")
    result = run(imre, "-m", "checked_operand", stdin=b"send\n")
    assert (result.returncode, result.stdout, result.stderr) == (0, b"8\n[3]\n", b""), result

    # The ordinary checker owns the error even when the entry lives in an archive.
    archive = root / "checked_operand.pyz"
    with zipfile.ZipFile(archive, "w") as zipped:
        zipped.writestr("__main__.py", before + "values.append(True)\n")
    result = run(imre, archive, stdin=b"send\n")
    assert result.returncode and not result.stdout, result
    assert b"__main__.py:9:15: Imre: expected int, got bool" in result.stderr, result

    program = (
        functions
        + "def install(unused): globals()['values'] = destination\n"
        + "def accept(self, value): print('accepted', value)\n"
        + setup
        + "destination = type('Destination', (), {'append': accept})()\n"
        + "previous = type('Previous', (), {'__del__': install})\n"
        + "values: list[int] = [3]\n"
    )
    for call in ("install(None)", "{'install': install}['install'](None)"):
        entry.write_text(program + call + "\nvalues.append(True)\n")
        result = run(imre, "-m", "checked_operand", stdin=b"send\n")
        assert (result.returncode, result.stdout) == (0, b"8\naccepted True\n"), result

    # Selecting object operations for an imported module leaves this local contract intact.
    (root / "checked_sibling.py").write_text(
        "import socket, threading\n"
        "def worker():\n"
        "    sock = socket.socket()\n"
        "    threading.Thread(target=sock.close).start()\n"
    )
    entry.write_text(
        "import checked_sibling\n"
        "def observe(value): pass\n"
        "values: list[int] = []\n"
        "observe(None)\n"
        "values.append(True)\n"
    )
    result = run(imre, "-m", "checked_operand", stdin=b"")
    assert result.returncode and not result.stdout, result
    assert b"checked_operand.py:5:15: Imre: expected int, got bool" in result.stderr, result

    # Inference can discover this file's concrete error before it solves the
    # signature for the later callback.
    entry.write_text(
        functions + "def observe(value): pass\n"
        "values: list[int] = [3]\nobserve(None)\nvalues.append(True)\n" + setup
    )
    result = run(imre, "-m", "checked_operand", stdin=b"send\n")
    assert result.returncode and not result.stdout, result
    assert b"Imre: expected int, got bool" in result.stderr, result

    bad_body = functions.replace("    send(a + b)", "    values: list[int] = [3]\n    values.append(True)\n    send(a + b)")
    replacements = (
        "def replacement(a: int, b: int, send):\n    print('replacement')\n    send(a + b)\n"
        "def install(callback):\n    globals()['publish'] = replacement\n    return callback\n"
        "send = {'send': report}[input()]\n"
    )
    for invocation in ("publish(3, 5, send)", "publish(3, 5, install(send))"):
        entry.write_text(bad_body + replacements + invocation + "\n")
        result = run(imre, "-m", "checked_operand", stdin=b"send\n")
        assert result.returncode and not result.stdout, result
        assert b"checked_operand.py:3:19: Imre: expected int, got bool" in result.stderr, result
    entry.write_text(bad_body + replacements + "install(send)\npublish(3, 5, send)\n")
    result = run(imre, "-m", "checked_operand", stdin=b"send\n")
    assert (result.returncode, result.stdout, result.stderr) == (0, b"replacement\n8\n", b""), result

    worker = root / "checked_worker.py"
    worker.write_text(bad_body)
    receive = "def report(value): print(value)\nsend = {'send': report}[input()]\n"
    for import_line, publish in (
        ("from checked_worker import publish as deliver", "deliver"),
        ("import checked_worker", "checked_worker.publish"),
    ):
        entry.write_text(import_line + "\n" + receive + f"{publish}(3, 5, send)\n")
        result = run(imre, "-m", "checked_operand", stdin=b"send\n")
        assert result.returncode and not result.stdout, result
        assert b"checked_worker.py:3:19: Imre: expected int, got bool" in result.stderr, result
    with zipfile.ZipFile(archive, "w") as zipped:
        zipped.writestr("__main__.py", entry.read_text())
        zipped.writestr(worker.name, bad_body)
    result = run(imre, archive, stdin=b"send\n")
    assert result.returncode and b"checked_worker.py:3:19" in result.stderr, result

    entry.write_text(
        "import checked_worker\n" + receive
        + "def replacement(a, b, send): send(a - b)\n"
        "checked_worker.publish = replacement\nchecked_worker.publish(5, 3, send)\n"
    )
    result = run(imre, "-m", "checked_operand", stdin=b"send\n")
    assert (result.returncode, result.stdout, result.stderr) == (0, b"2\n", b""), result

    # Moving an inert function declaration past a store changes which initializer
    # lowers the store, while Python still looks up the callee before its arguments.
    entry.write_text(
        "import checked_worker\n"
        "def replacement(a, b, send): send(a - b)\n"
        "def report(value): print(value)\n"
        "handlers = {'send': report}\nsend = handlers[input()]\n"
        "def install(f):\n    checked_worker.publish = replacement\n    return f\n"
        "checked_worker.publish(3, 5, install(send))\n"
    )
    result = run(imre, "-m", "checked_operand", stdin=b"send\n")
    assert result.returncode and not result.stdout, result
    assert b"checked_worker.py:3:19: Imre: expected int, got bool" in result.stderr, result

    entry.write_text(
        bad_body + "def abort(_): raise LookupError('argument stopped')\n"
        "send = {'send': report}[input()]\npublish(3, 5, abort(send))\n"
    )
    result = run(imre, "-m", "checked_operand", stdin=b"send\n")
    assert result.returncode == 1 and b"LookupError: argument stopped" in result.stderr, result


    # Replacing a published annotation must still let a Python method receiver reach its body.
    entry.write_text(
        "START = 'a'\nEND = 'b'\n"
        "def pair(readme: str) -> int:\n"
        "    start = readme.find(START)\n    return readme.find(END, start)\n"
        "print(pair('ab'))\n"
        "def startup(source: str) -> None: exec(source)\n"
        "exec(\"pair.__annotations__ = {'readme': int}\\ntry: pair(12)\\n"
        "except Exception as exc: print(type(exc).__name__)\")\n"
    )
    result = run(imre, "-m", entry.stem)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"1\nAttributeError\n", b""), result

    entry.write_text(
        "def label(value: str) -> str: return f'L:{value}'\n"
        "print(label('ab'))\n"
        "def startup(source: str) -> None: exec(source)\n"
        "exec(" + repr(
            "label.__annotations__ = {'value': int}\nprint(label(12))\n"
            "del label.__annotations__\nprint(label(13))"
        ) + ")\n"
    )
    result = run(imre, "-m", entry.stem)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"L:ab\nL:12\nL:13\n", b""), result


def check_installed_package_data(imre, root, run):
    site = root / "invoice-site"
    package = site / "prices"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("from .rates import price\n")
    (package / "rates.py").write_text("price = 63\n")
    data = package / "currency"
    data.write_bytes(b"EUR")
    metadata = site / "prices-1.0.dist-info"
    metadata.mkdir()
    (metadata / "RECORD").write_text("prices/__init__.py,,\nprices/rates.py,,\nprices/currency,,\n")
    entry = root / "read_price.py"
    entry.write_text(
        "from importlib.resources import files\nimport prices\n"
        "print(prices.price)\nprint(files(prices).joinpath('currency').read_bytes())\n"
    )
    exe = root / "read-price"
    built = run(imre, "build", entry, "--extension-path", site, "--opt-level", "0", "-o", exe,
                "--no-cache", timeout=300)
    assert built.returncode == 0, built.stderr
    entry.unlink()
    shutil.rmtree(site)
    missing = run(exe)
    assert (missing.returncode, missing.stdout) == (1, b"63\n"), missing
    assert b"FileNotFoundError" in missing.stderr and os.fsencode(data) in missing.stderr, missing
    package.mkdir(parents=True)
    data.write_bytes(b"USD")
    result = run(exe)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"63\nb'USD'\n", b""), result


def check_module_search_root(imre, root, run):
    app = root / "application"
    app.mkdir()
    (app / "wheel_module.py").write_text(
        "def twice(n: int) -> int: return n * 2\n"
        "def unused() -> int: return twice('x')\n"
        "scope = {}\n"
        "exec('def echo(n: int): return n', scope)\n"
        "assert scope['echo']('text') == 'text'\n"
    )
    distribution = app / "wheel_module-1.dist-info"
    distribution.mkdir()
    (distribution / "RECORD").write_text("wheel_module.py,,\n")
    scores = app / "scores.py"
    scores.write_text(
        "def take(n: int, other: int) -> int: return n + other\n"
        "def later(): raise LookupError('second argument')\n"
        "try: take('x', later())\n"
        "except LookupError as error: print(error.args[0])\n"
    )
    entry = app / "launch.py"
    entry.write_text("import scores, wheel_module\nprint(wheel_module.twice(scores.take(1, 2)))\n")
    result = run(imre, "-B", "-m", "launch", extra={"PYTHONPATH": str(app)})
    assert (result.returncode, result.stdout, result.stderr) == (0, b"second argument\n6\n", b""), result

    model = "def model() -> int:\n    values: list[int] = [3]\n    values.append('bad')\n    return values[0]\n"
    entry.write_text(model + "print('reached')\nprint(model())\n")
    result = run(imre, "-B", "-m", "launch", extra={"PYTHONPATH": str(app)})
    assert (result.returncode, result.stdout, result.stderr) == (0, b"reached\n3\n", b""), result

    entry.write_text(model + "print(model())\n")
    result = run(imre, "-B", "-m", "launch", extra={"PYTHONPATH": str(app)})
    assert result.returncode and not result.stdout, result
    assert os.fsencode(entry) + b":3:19: Imre: expected int, got str" in result.stderr, result

    scores.write_text("def take(n: int): return n\ntake('wrong')\n")
    entry.write_text("import scores\n" + model + "print('reached')\nprint(model())\n")
    result = run(imre, "-B", "-m", "launch", extra={"PYTHONPATH": str(app)})
    assert result.returncode and not result.stdout, result
    assert os.fsencode(scores) + b":2:6: Imre: expected int, got str" in result.stderr, result

    # The entry comes from app even when the ordinary lookup of scores prefers the wheel.
    wheel = root / "score-wheel"
    wheel.mkdir()
    (wheel / "scores.py").write_text("def take(n: int): return n\n")
    metadata = wheel / "scores-1.dist-info"
    metadata.mkdir()
    (metadata / "RECORD").write_text("scores.py,,\n")
    scores.write_text("def take(n: int): return n\n")
    entry.write_text(
        "import sys\nsys.path.insert(0, sys.argv[1])\nfrom scores import take\n"
        "try: print(take(sys.argv[2]))\nexcept TypeError as error: print(error)\n"
    )
    search = {"PYTHONPATH": os.pathsep.join(map(str, (wheel, app)))}
    for selected, expected in ((app, b'take argument "n" requires int\n'), (wheel, b"x\n")):
        result = run(imre, "-B", "-m", "launch", str(selected), "x", extra=search)
        assert (result.returncode, result.stdout, result.stderr) == (0, expected, b""), result

    # Overlapping roots expose the same file as an application module and as a provider.
    back = app / "back"
    back.mkdir()
    (back / "scores.py").write_text("def take(n: int): return n\n")
    entry.write_text(
        "import sys\nfrom back import scores as nested\n"
        "sys.path.insert(0, sys.argv[1])\nimport scores\n"
        "for module in (nested, scores):\n"
        "    try: print(module.take(sys.argv[2]))\n"
        "    except TypeError as error: print(error)\n"
    )
    search = {"PYTHONPATH": os.pathsep.join(map(str, (app, back)))}
    result = run(imre, "-B", "-m", "launch", str(back), "x", extra=search)
    assert (result.returncode, result.stdout, result.stderr) == (0, b'take argument "n" requires int\nx\n', b""), result

    # runpy imports the parent into its own namespace before entering the body.
    package = app / "entry_package"
    package.mkdir()
    (package / "__init__.py").write_text(
        "def __getattribute__(n: int): return n\n"
        "from __main__ import __getattribute__ as loaded\n"
        "print(loaded('__name__'))\n"
    )
    (package / "__main__.py").write_text("print(__spec__.name)\n")
    result = run(imre, "-B", "-m", "entry_package", extra={"PYTHONPATH": str(app)})
    assert (result.returncode, result.stdout, result.stderr) == (0, b"__main__\nentry_package.__main__\n", b""), result


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


def check_installed_site(imre, root, run):
    # A real relocated launcher keeps this test independent of the developer's installation.
    prefix = root.resolve() / "prefix"
    install = prefix / "bin"
    install.mkdir(parents=True)
    launcher = Path(shutil.copy2(imre, install / "imre"))
    for name in ("libimre_runtime.a", "imre-toolchain"):
        companion = imre.with_name(name)
        if companion.exists():
            (install / name).symlink_to(companion, target_is_directory=companion.is_dir())
    credentials = root / "credentials.netrc"
    credentials.write_text("machine example.test login user password sample\n")
    authenticated = run(
        launcher, "-IB", "-c",
        "import netrc, sys; print(netrc.netrc(sys.argv[1]).authenticators('example.test'))",
        credentials, timeout=300,
    )
    assert (authenticated.returncode, authenticated.stdout, authenticated.stderr) == (
        0, b"('user', '', 'sample')\n", b"",
    ), authenticated
    name = "xml.etree.ElementTree"
    sample = root / "sample.tar"
    member = root / "payload"
    member.write_bytes(b"archive member")
    with tarfile.open(sample, "w", format=tarfile.PAX_FORMAT, pax_headers={"comment": "sample"}) as tar:
        tar.add(member, arcname="payload")
    use_sources = (
        "import getpass, importlib, os, sys, tarfile\n"
        "os.environ['LOGNAME'] = 'archive-user'\n"
        "assert getpass.getuser() == 'archive-user'\n"
        "print(importlib.import_module(sys.argv[1]).Element('root').tag)\n"
        "sorter = importlib.import_module(sys.argv[3]).TopologicalSorter\n"
        "assert importlib.import_module(sys.argv[4]).mean([2, 3, 4]) == 3\n"
        "assert importlib.import_module(sys.argv[5]).unescape('&lt;&amp;&gt;') == '<&>'\n"
        "assert importlib.import_module(sys.argv[6]).Wave_read.__name__ == 'Wave_read'\n"
        "address = importlib.import_module(sys.argv[7]).ip_address('2001:0db8::1')\n"
        "assert str(address) == '2001:db8::1'\n"
        "cookie = importlib.import_module(sys.argv[8]).SimpleCookie('session=abc; Path=/')\n"
        "assert cookie['session'].value == 'abc' and cookie['session']['path'] == '/'\n"
        "cookie['note'] = 'a,b'\n"
        "assert cookie['note'].coded_value == r'\"a\\054b\"'\n"
        "assert list(sorter({'test': ('build',)}).static_order()) == ['build', 'test']\n"
        "with tarfile.open(sys.argv[2], 'r:') as tar:\n"
        "    with tar.extractfile('payload') as member:\n"
        "        assert member.read() == b'archive member'\n"
    )
    source_args = (name, sample, "graphlib", "statistics", "html", "wave", "ipaddress", "http.cookies")
    archive = install / "imre-stdlib.zip"
    # The relocated launcher works both on its own and in a release installation.
    bare = run(launcher, "-IB", "-c", use_sources, *source_args, timeout=300)
    assert (bare.returncode, bare.stdout, bare.stderr) == (0, b"root\n", b""), bare
    opaque = run(
        launcher, "-IB", "-c", "import _imp, sys; exec(sys.argv[1])",
        "import xml; print(xml.__name__)", timeout=300,
    )
    assert (opaque.returncode, opaque.stdout, opaque.stderr) == (0, b"xml\n", b""), opaque
    fallback = run(
        launcher, "-IB", "-c",
        "import sys; sys.modules['_datetime'] = None; from datetime import date; "
        "print(date(2000, 2, 29).isoformat())", timeout=300,
    )
    assert (fallback.returncode, fallback.stdout, fallback.stderr) == (0, b"2000-02-29\n", b""), fallback
    assert not archive.exists()
    exported = run(launcher, "--emit-bundled-stdlib")
    assert exported.returncode == 0 and not exported.stderr, exported
    archive.write_bytes(exported.stdout)
    packaged = run(launcher, "-IB", "-c", use_sources, *source_args, timeout=300)
    assert (packaged.returncode, packaged.stdout, packaged.stderr) == (0, b"root\n", b""), packaged
    words = run(
        launcher, "-IB", "-c",
        "import importlib, sys; lexer = importlib.import_module(sys.argv[1]); "
        "print(lexer.join(lexer.split(sys.argv[2])))",
        "shlex", "'two words' tail", timeout=300,
    )
    assert (words.returncode, words.stdout, words.stderr) == (0, b"'two words' tail\n", b""), words
    command = run(
        launcher, "-IB", "-c",
        "import importlib, sys; shell = importlib.import_module(sys.argv[1]).Cmd(); "
        "print(shell.parseline('?hello'))",
        "cmd", timeout=300,
    )
    assert (command.returncode, command.stdout, command.stderr) == (0, b"('help', 'hello', 'help hello')\n", b""), command
    reader = root / "read_tar_entry.py"
    reader.write_text(
        "from graphlib import TopologicalSorter\n"
        "from tarfile import open as open_archive\n"
        "assert list(TopologicalSorter({'test': ('build',)}).static_order()) == ['build', 'test']\n"
        f"with open_archive({str(sample.resolve())!r}, 'r:') as archive:\n"
        "    with archive.extractfile('payload') as member:\n"
        "        print(member.read().decode('ascii'))\n"
    )
    direct = run(launcher, "-B", reader, timeout=300)
    assert (direct.returncode, direct.stdout, direct.stderr) == (0, b"archive member\n", b""), direct
    shadow = root / "shadow_xml"
    shadow.mkdir()
    (shadow / "xml.py").write_text("marker = 73\n")
    probe = shadow / "probe.py"
    probe.write_text("import importlib, sys\nprint(importlib.import_module(sys.argv[1]).marker)\n")
    local = run(launcher, "-B", probe, "xml", timeout=300)
    assert (local.returncode, local.stdout, local.stderr) == (0, b"73\n", b""), local
    # The absent registered child must not pull its sibling into the local package.
    etree = root / "regular_parent" / "xml" / "etree"
    etree.mkdir(parents=True)
    (etree.parent / "__init__.py").write_text("")
    (etree / "__init__.py").write_text("")
    (etree / "ElementPath.py").write_text("def select(\n")
    consumer = etree.parent.parent / "read_xml.py"
    consumer.write_text(
        "print('start', flush=True)\n"
        "try:\n"
        "    import xml.etree.ElementTree\n"
        "except ModuleNotFoundError as error:\n"
        "    print(error.name)\n"
    )
    missing = run(launcher, "-B", consumer, timeout=300)
    assert (missing.returncode, missing.stdout, missing.stderr) == (0, b"start\nxml.etree.ElementTree\n", b""), missing
    target = run(launcher, "build-target")
    assert target.returncode == 0, target.stderr
    version = json.loads(target.stdout)["python_version"]
    site = prefix / "lib" / f"imre{version}" / "site-packages"
    use_decimal = (
        "import importlib.util, sys; from fractions import Fraction; "
        "decimal = importlib.import_module(sys.argv[1]); "
        "second = importlib.util.module_from_spec(decimal.__spec__); "
        "decimal.__loader__.exec_module(second); "
        "assert (decimal.Decimal('1') == '1') is False and (second.Decimal('1') != '1') is True; "
        "print(decimal.Decimal(10), second.Decimal(11), Fraction.from_decimal(10))"
    )
    for installed in (False, True):
        if installed:
            site.mkdir(parents=True)
        result = run(launcher, "-B", "-c", use_decimal, "decimal", timeout=300)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"10 11 10\n", b""), result
    package = site / "installed_example"
    package.mkdir(parents=True)
    init = package / "__init__.py"
    init.write_text("def value(): return 17\n")
    script = root / "read_installed.py"
    script.write_text("from installed_example import value; print(value())\n")
    (package / "__main__.py").write_bytes(script.read_bytes())
    for args in (("-B", "-c", script.read_text()), ("-IB", script), ("-IB", "-m", package.name)):
        result = run(launcher, *args, timeout=300)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"17\n", b""), result
    no_site = (
        "import sys\n"
        "print(sys.flags.no_site, sys.argv[1] in sys.path, sys.argv[2] in sys.path)\n"
        "try:\n"
        "    import installed_example\n"
        "except ModuleNotFoundError as error:\n"
        "    print(error.name)\n"
    )
    result = run(launcher, "-IS", "-c", no_site, str(site), str(launcher / "<imre stdlib>"), timeout=300)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"1 False True\ninstalled_example\n", b""), result
    init.write_text("def value(): return 23\n")
    result = run(launcher, "-B", "-c", script.read_text(), timeout=300)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"23\n", b""), result
    # Files installed in the standard library are available before site runs.
    tariff = site.parent / "installed_tariff.py"
    tariff.write_text("def freight(weight: int) -> int: return weight * 2\n")
    static = "from installed_tariff import freight; print(freight(3))"
    runtime = "import importlib, sys; print(importlib.import_module(sys.argv[1]).freight(3))"
    for args in (("-ESB", "-c", static), ("-ISB", "-c", runtime, tariff.stem)):
        result = run(launcher, *args, timeout=300)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"6\n", b""), result
    check_symlinked_venv(launcher, site, root.resolve(), run)
    check_copied_venv(launcher, root.resolve(), run)
    # Discovering a filesystem library must not switch a checked program's backend.
    (site.parent / "sitecustomize.py").write_text("print('stdlib hook')\n")
    checked = run(launcher, "-EB", "-c", static, timeout=300)
    assert (checked.returncode, checked.stdout, checked.stderr) == (0, b"6\n", b""), checked
    # An object program already starts the shared interpreter, so its hook runs once.
    boxed = run(launcher, "-EB", "-c", runtime, tariff.stem, timeout=300)
    assert (boxed.returncode, boxed.stdout, boxed.stderr) == (0, b"stdlib hook\n6\n", b""), boxed


def check_copied_venv(launcher, root, run):
    def create(executable, target):
        result = run(executable, "-B", "-m", "venv", "--without-pip", "--copies", target, timeout=300)
        assert result.returncode == 0, result.stderr
        assert not (target / "bin" / "python").is_symlink()

    def check(env):
        python = env / "bin" / "python"
        result = run(
            python, "-B", "-c",
            "import sys; print(sys.implementation.name, sys.executable, sys._base_executable, "
            "sys.prefix, sys.base_prefix, sep='\\n')", timeout=300,
        )
        assert result.returncode == 0, result.stderr
        expected = ["imre", str(python), str(launcher), str(env), str(launcher.parent.parent)]
        assert result.stdout.decode().splitlines() == expected

    copied = root / "copied-venv"
    create(launcher, copied)
    check(copied)
    alias = root / "copied-venv-link"
    alias.symlink_to(copied, target_is_directory=True)
    check(alias)
    nested = root / "nested-copied-venv"
    create(copied / "bin" / "python", nested)
    copied.rename(root / "moved-outer-venv")
    check(nested)


def check_symlinked_venv(launcher, base_site, root, run):
    base = launcher.parent.parent
    venv = root / "symlink-venv"
    link = venv / "bin" / "venv-python"
    link.parent.mkdir(parents=True)
    link.symlink_to(launcher)
    local_site = venv / base_site.relative_to(base)
    local_site.mkdir(parents=True)
    decoy = root / "decoy-bin"
    decoy.mkdir()
    (decoy / link.name).write_text("not executable\n")
    env = {"PATH": os.pathsep.join([str(decoy), str(link.parent), os.environ["PATH"]])}
    code = (
        "import importlib.util, site, sys\n"
        "print(sys.executable == sys.argv[1], sys._base_executable == sys.argv[2], "
        "sys.prefix == sys.exec_prefix == sys.argv[3], sys.base_prefix == sys.base_exec_prefix == sys.argv[4])\n"
        "print(site.getsitepackages() == sys.argv[5:], site.ENABLE_USER_SITE, "
        "importlib.util.find_spec('installed_example') is not None)\n"
    )
    for enabled, flag in [(True, "-sB"), (False, "-B")]:
        (venv / "pyvenv.cfg").write_text(
            f"home = {launcher.parent}\ninclude-system-site-packages = {str(enabled).lower()}\n"
        )
        expected_sites = [local_site, base_site] if enabled else [local_site]
        result = run(link.name, flag, "-c", code, link, launcher, venv, base, *expected_sites,
                     extra=env, timeout=300)
        assert (result.returncode, result.stdout, result.stderr) == (
            0, b"True True True True\nTrue False " + str(enabled).encode() + b"\n", b"",
        ), result
    # -S suppresses site-packages but preserves the base standard library in a venv.
    code = (
        "import sysconfig; from installed_tariff import freight; "
        "print(sysconfig.get_path('stdlib'), freight(4))"
    )
    result = run(link, "-ISB", "-c", code, timeout=300)
    assert (result.returncode, result.stdout, result.stderr) == (
        0, f"{base_site.parent} 8\n".encode(), b"",
    ), result


def check_sysconfig_build(imre, root, run):
    by_name = (
        "import _imp, importlib, sys\n"
        "mod = importlib.import_module(sys.argv[1])\n"
        "config_vars = mod._CONFIG_VARS\n"
        "version = mod.get_python_version()\n"
        "assert version == '%d.%d' % sys.version_info[:2]\n"
        "assert mod.get_python_version() is version\n"
        "assert mod._CONFIG_VARS is config_vars\n"
        "assert mod.get_config_var('EXT_SUFFIX') in _imp.extension_suffixes()\n"
        "assert mod.get_config_vars() is mod.get_config_vars()\n"
    )
    flags = "\x1f".join(["--no-cache", "--opt-level", "0"])
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


def check_user_site(imre, root, run):
    root = root.resolve()
    version = json.loads(run(imre, "build-target").stdout)["python_version"]
    base = root / "user_base"
    packages = base / "lib" / f"python{version}" / "site-packages"
    packages.mkdir(parents=True)
    (packages / "user_site_probe.py").write_text("value = 73\n")
    extras = root / "user_extras"
    extras.mkdir()
    (extras / "pth_probe.py").write_text("value = 51\n")
    (packages / "extras.pth").write_text(str(extras) + "\n")
    extra = {"PYTHONUSERBASE": str(base)}
    code = (
        "import sys\n"
        "assert 'site' in sys.modules\n"
        "prefix = sys.prefix\n"
        "sys.prefix = 'after startup'\n"
        "import site\n"
        "assert site.PREFIXES[:2] == [prefix, sys.exec_prefix]\n"
        "sys.prefix = prefix\n"
        "import importlib.util\n"
        "print(site.USER_BASE == site.getuserbase() == sys.argv[1], "
        "site.USER_SITE == site.getusersitepackages() == sys.argv[2])\n"
        "print(site.ENABLE_USER_SITE, sys.flags.no_user_site, "
        "importlib.util.find_spec('user_site_probe') is not None, "
        "importlib.util.find_spec('pth_probe') is not None)\n"
    )
    for flags, expected in [("-B", b"True 0 True True"), ("-sB", b"False 1 False False")]:
        result = run(imre, flags, "-c", code, str(base), str(packages), extra=extra, timeout=300)
        assert (result.returncode, result.stdout, result.stderr) == (
            0, b"True True\n" + expected + b"\n", b"",
        ), result
    imported = run(imre, "-B", "-c", "import user_site_probe; print(user_site_probe.value)", extra=extra, timeout=300)
    assert (imported.returncode, imported.stdout, imported.stderr) == (0, b"73\n", b""), imported
    lazy = run(
        imre, "-SB", "-c",
        "import sys\n"
        "assert 'site' not in sys.modules\n"
        "sys.prefix = 'before import'\n"
        "import site\n"
        "sys.prefix = 'after import'\n"
        "assert site.PREFIXES == ['before import', sys.exec_prefix]\n"
        "print(site.USER_BASE, site.USER_SITE, site.ENABLE_USER_SITE)\n"
        "print(site.getuserbase() == sys.argv[1], site.getusersitepackages() == sys.argv[2], sys.argv[2] in sys.path)",
        str(base), str(packages), extra=extra, timeout=300,
    )
    assert (lazy.returncode, lazy.stdout, lazy.stderr) == (0, b"None None None\nTrue True False\n", b""), lazy


def check_script_sitecustomize(imre, root, run):
    root = root.resolve()
    app = root / "script-app"
    hooks = root / "startup-hooks"
    packages = root / "startup-packages"
    for directory in (app, hooks, packages):
        directory.mkdir()
    # The hook runs before the script directory enters sys.path. After a failed
    # import the script can import a different file under the same module name.
    (app / "sitecustomize.py").write_text("print('script local')\n")
    (app / "startup_helper.py").write_text("def (\n")
    (hooks / "startup_helper.py").write_text("print('installed helper')\n")
    (hooks / "sitecustomize.py").write_text(
        "import startup_helper\nraise RuntimeError('setup failed')\n"
    )
    script = app / "reimport.py"
    script.write_text("import sitecustomize\nprint('entry')\n")
    result = run(imre, "-B", script, extra={"PYTHONPATH": str(hooks)}, timeout=300)
    assert (result.returncode, result.stdout) == (0, b"installed helper\nscript local\nentry\n"), result
    assert b"RuntimeError: setup failed" in result.stderr, result

    (app / "sitecustomize.py").write_text("print('script-local hook ran')\n")
    (hooks / "sitecustomize.py").write_text(f"import sys\nsys.path.insert(0, {str(packages)!r})\n")
    (packages / "startup_plugin.py").write_text("value = 'plugin loaded'\n")
    script = app / "entry.py"
    script.write_text(
        "import importlib, sys\n"
        "print(sys.path[0] == sys.argv[1], sys.argv[2] in sys.path)\n"
        "try:\n"
        "    print(importlib.import_module(sys.argv[3]).value)\n"
        "except ModuleNotFoundError as error:\n"
        "    print(error.name)\n"
    )
    env = {"PYTHONPATH": str(hooks)}
    for flag, stdout in [
        ("-B", b"True True\nplugin loaded\n"),
        ("-BS", b"True True\nstartup_plugin\n"),
        ("-BE", b"True False\nstartup_plugin\n"),
    ]:
        result = run(
            imre, flag, script, app, hooks, "startup_plugin", extra=env, timeout=300,
        )
        assert (result.returncode, result.stdout, result.stderr) == (0, stdout, b""), result

    unfrozen = root / "unfrozen-providers"
    unfrozen.mkdir()
    (unfrozen / "stat.py").write_text("raise AssertionError('unexpected startup import')\n")
    invoice = app / "invoice.py"
    invoice.write_text("import sys\nprint('site' in sys.modules, 'stat' in sys.modules)\n")
    result = run(
        imre, "-B", "-X", "frozen_modules=off", invoice,
        extra={"PYTHONPATH": str(unfrozen)}, timeout=300,
    )
    assert (result.returncode, result.stdout, result.stderr) == (0, b"False False\n", b""), result

    literal = app / "literal_import.py"
    literal.write_text("from startup_plugin import value\nprint(value)\n")
    result = run(imre, "-B", literal, extra=env, timeout=300)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"plugin loaded\n", b""), result

    invalid = app / "typed_argument.py"
    invalid.write_text(
        "def take(value: int) -> int:\n    return value\n"
        "\ndef dormant() -> int:\n    return take('bad')\n"
        "\nprint(1)\n"
    )
    result = run(imre, "-B", invalid, extra=env)
    assert result.returncode > 0 and not result.stdout and b"expected int, got str" in result.stderr, result

    # The hook fails during site initialization, before application shutdown is available.
    (hooks / "sitecustomize.py").write_text("print('starting')\nraise SystemExit(19)\n")
    script.write_text("import sitecustomize\nprint('application')\n")
    fatal = run(imre, "-B", script, extra=env)
    assert fatal.returncode == 1 and fatal.stdout == b"starting\n", fatal
    assert b"Fatal Python error: init_import_site:" in fatal.stderr, fatal
    assert fatal.stderr.endswith(b"SystemExit: 19\n"), fatal


def check_explicit_site_directory(imre, root, run):
    site_dir = root / "extra-site"
    plugins = site_dir / "plugins"
    plugins.mkdir(parents=True)
    (plugins / "activated_plugin.py").write_text("value = 41\n")
    (site_dir / "plugins.pth").write_text("plugins\nimport sys; sys.pth_increment = 1\n")
    source = (
        "import importlib, sys\n"
        "site = importlib.import_module(sys.argv[1])\n"
        "known = set()\n"
        "assert site.addsitedir(sys.argv[2], known_paths=known) is known\n"
        "import activated_plugin\n"
        "print(activated_plugin.value + sys.pth_increment)\n"
    )
    result = run(imre, "-B", "-c", source, "site", str(site_dir), timeout=300)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"42\n", b""), result


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
        built = run(imre, "build", imported_fixture(root, source), "--no-cache", "--opt-level", str(optimization), "-o", executable,
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
        built = run(imre, "build", source, "--no-cache", "--opt-level", str(optimization), "-o", executable,
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
        built = run(imre, "build", imported_fixture(root, source), "--no-cache", "--opt-level", str(optimization), "-o", executable,
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
        "-C", "link-arg=-Wl,--wrap=imre_exception_at_frame,--wrap=malloc",
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
    built = run(imre, "build", source, "--no-cache", "-o", executable, extra={
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
        "-C", "link-arg=-Wl,--wrap=imre_dyn_scoped_call_at,--wrap=imre_dyn_op_at,--wrap=imre_dyn_lookup,--wrap=imre_dyn_assign_target,--wrap=malloc,--wrap=realloc,--wrap=imre_dyn_lit,--wrap=imre_dyn_linked_string",
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
                "IMRE_TEST_CALL_OP": "assign_target" if operation == "assign" else operation,
                "IMRE_TEST_CALL_SIZE": str(size),
                "IMRE_TEST_GETTER_ALLOCATION": "1",
            })
            # Both operations borrow the linked identifier without allocating a copy.
            if operation == "lookup":
                expected = b"value 3\nretry 3\n"
            else:
                expected = b"value 7\npreserved True\nretry 3\n"
            assert (result.returncode, result.stdout, result.stderr) == (
                0, expected, b"",
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
        "-C", "link-arg=-Wl,--wrap=imre_dyn_lookup,--wrap=realloc",
    ])
    import_path = f"import sys; sys.path.insert(0, {str(fixtures)!r}); "
    source = import_path + "import boxed_ffi_result_roots as fixture; fixture.allocation()"
    for ordinal in (0, 1):
        result = run(imre, "-B", "-c", source, extra={
            "IMRE_ENCODED_FLAGS": result_flags,
            "IMRE_TEST_GETTER_ALLOCATION": str(ordinal),
        }, timeout=300)
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


def check_os_execvpe(imre, root, run):
    fixture = Path(__file__).resolve().with_name("fixtures") / "boxed_os_execvpe.py"
    executable = root / "execvpe"
    built = run(imre, "build", fixture, "--opt-level", "0", "-o", executable, timeout=300)
    assert built.returncode == 0, built
    for mode in ("errors", "bindings", "inherited", "supplied", "slash"):
        expected = (0, b"", b"") if mode in ("errors", "bindings") else (77, b"replaced\n", b"")
        for command in ((sys.executable, "-IB", fixture), (executable,)):
            result = run(*command, mode)
            assert (result.returncode, result.stdout, result.stderr) == expected, (mode, result)


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


def check_tracemalloc_startup(imre, run):
    probe = (
        "import sys, tracemalloc; "
        "print(sys._xoptions, tracemalloc.is_tracing(), tracemalloc.get_traceback_limit())"
    )
    for flags, setting in (
        (("-X", "tracemalloc=2", "-X", "tracemalloc=5"), "3"),
        (("-E",), "invalid"),
        (("-I", "-X", "tracemalloc"), "invalid"),
        (("-X", "tracemalloc=1"), "invalid"),
        (("-X", "tracemalloc=-1"), ""),
    ):
        env = {"PYTHONTRACEMALLOC": setting}
        expected = run(sys.executable, *flags, "-c", probe, extra=env)
        actual = run(imre, *flags, "-c", probe, extra=env, timeout=600)
        assert (actual.returncode, actual.stdout, actual.stderr) == (
            expected.returncode, expected.stdout, expected.stderr
        )

    # Reject an out-of-range runtime limit before reading or compiling the entry.
    if sys.platform.startswith("linux"):
        for args in (("-c", "if:"), ("missing-tracemalloc-script.py",)):
            expected = run(sys.executable, "-Xtracemalloc=65536", *args)
            actual = run(imre, "-Xtracemalloc=65536", *args)
            assert actual.returncode == expected.returncode == 1 and actual.stdout == expected.stdout == b""
            # The fatal diagnostic includes the reporting process's thread ID and name.
            assert actual.stderr.partition(b"Current thread 0x")[0] == expected.stderr.partition(b"Current thread 0x")[0]
            assert actual.stderr.endswith(b"  <no Python frame>\n"), actual.stderr

    # The ordinary compiler enforces annotated return types before selecting the traced runtime.
    wrong_return = "def f() -> int:\n    return 'bad'\nprint(f())"
    ordinary = run(imre, "-c", wrong_return)
    traced = run(imre, "-Xtracemalloc=1", "-c", wrong_return)
    assert traced.returncode == ordinary.returncode == 1
    assert b"annotations are enforced contracts" in ordinary.stderr and traced.stderr == ordinary.stderr

    # This import normally specializes to the typed representation.
    spec = "from importlib.machinery import ModuleSpec; print(ModuleSpec('sample', None).name)"
    traced = run(imre, "-Xtracemalloc=1", "-c", spec, timeout=600)
    assert (traced.returncode, traced.stdout, traced.stderr) == (0, b"sample\n", b"")


def check_registered_run_path(imre, root, run):
    project = root / "reexecute"
    for name, rate in (("wholesale", 7), ("retail", 11)):
        package = project / name
        package.mkdir(parents=True)
        (package / "__init__.py").touch()
        (package / "rates.py").write_text(f"rate = {rate}\n")
    invoice = project / "wholesale" / "invoice.py"
    invoice.write_text(
        "from __future__ import annotations\n"
        "from .rates import rate\n"
        "def quote() -> int:\n"
        "    assert 'annotations' in globals()\n"
        "    return rate * globals().get('quantity', 1)\n"
    )
    entry = project / "report.py"
    entry.write_text(
        "import sys\n"
        "from importlib import import_module\n"
        "import wholesale.invoice\n"
        "import retail.rates\n"
        "def veto(name, g, local, fromlist, level):\n"
        "    seen.append((name, fromlist, level))\n"
        "    if name == '__future__': return __import__(name, g, local, fromlist, level)\n"
        "    raise PermissionError(name)\n"
        "runpy = import_module('runpy')\n"
        "quotes = []\n"
        "for package, quantity in [(wholesale.invoice.__package__, 2), (retail.rates.__package__, 3)]:\n"
        f"    result = runpy.run_path({str(invoice)!r}, {{'quantity': quantity}}, package + '.report')\n"
        "    quotes.append(result['quote'])\n"
        "assert quotes[0].__code__ is not quotes[1].__code__\n"
        "seen = []\n"
        f"try: runpy.run_path({str(invoice)!r}, {{'__builtins__': {{'__import__': veto}}}}, 'retail.report')\n"
        "except PermissionError as exc: assert exc.args == ('rates',)\n"
        "assert seen == [('__future__', ('annotations',), 0), ('rates', ('rate',), 1)]\n"
        "print(wholesale.invoice.quote(), quotes[0](), quotes[1]())\n"
    )
    application = project / "report"
    build = run(imre, "build", entry, "--module-root", project, "-o", application, timeout=300)
    assert build.returncode == 0, build
    for command in ((sys.executable, "-BS", entry), (application,)):
        result = run(*command)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"7 14 33\n", b""), result

    declared = project / "declared"
    build = run(imre, "build", entry, "--module-root", project,
                "--include-module", "wholesale.invoice", "-o", declared, timeout=300)
    assert build.returncode == 0, build
    invoice.unlink()
    result = run(declared)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"7 14 33\n", b""), result
    missing = run(application)
    assert missing.returncode == 1 and b"FileNotFoundError" in missing.stderr, missing


def check_frozen_modules_startup(imre, root, run):
    provider = root / "frozen_provider"
    provider.mkdir()
    (provider / "__hello__.py").write_text("initialized = 73\ndef origin(): return __file__\n")
    entry = provider / "entry.py"
    entry.write_text(
        "import _imp, importlib, sys\n"
        "from importlib.machinery import FrozenImporter\n"
        "name = '__hello__'\n"
        "for override in (0, 1, -1, 0):\n"
        "    _imp._override_frozen_modules_for_tests(override)\n"
        "    sys.modules.pop(name, None)\n"
        "    import __hello__\n"
        "    assert importlib.import_module(name) is __hello__ is sys.modules[name]\n"
        "    frozen = _imp.is_frozen(name)\n"
        "    assert (__hello__.__loader__ is FrozenImporter) == frozen\n"
        "    assert __hello__.__spec__.loader is __hello__.__loader__\n"
        "    assert _imp.is_frozen('_frozen_importlib')\n"
        "    if not frozen:\n"
        "        assert __hello__.origin() == __hello__.origin.__code__.co_filename == __hello__.__file__\n"
        "    print(frozen, sys._xoptions)\n"
    )
    for flags, setting in (
        (("-X", "frozen_modules=off"), "on"),
        (("-Xfrozen_modules=off", "-Xfrozen_modules=on"), "off"),
        ((), "off"),
        (("-E",), "invalid"),
    ):
        extra = {"PYTHON_FROZEN_MODULES": setting}
        expected = run(sys.executable, "-B", *flags, entry, extra=extra)
        actual = run(imre, "-B", *flags, entry, extra=extra, timeout=300)
        assert expected.returncode == actual.returncode == 0, (expected, actual)
        assert actual.stdout == expected.stdout and actual.stderr == expected.stderr == b""

    # A scalar from-import also resolves the local file through the checked source graph.
    direct = provider / "direct.py"
    direct.write_text("from __hello__ import initialized\nprint(initialized)\n")
    result = run(imre, "-BS", "-Xfrozen_modules=off", direct)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"73\n", b""), result

    # The module-mode entry point imports runpy before invoking the target.
    (provider / "runpy.py").write_text(
        "def _run_module_as_main(name, alter_argv=True): print(name, alter_argv)\n"
    )
    result = run(imre, "-BS", "-Xfrozen_modules=off", "-m", "direct", extra={"PYTHONPATH": str(provider)})
    assert (result.returncode, result.stdout, result.stderr) == (0, b"direct True\n", b""), result


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

    source = root / "user-fault.py"
    source.write_text(
        "import faulthandler, os, signal\n"
        "calls = []\n"
        "def previous(sig, frame): calls.append(sig)\n"
        "def deliver(): os.kill(os.getpid(), signal.SIGUSR1)\n"
        "signal.signal(signal.SIGUSR1, previous)\n"
        "faulthandler.register(signal.SIGUSR1, all_threads=False, chain=True)\n"
        "assert not faulthandler.is_enabled()\n"
        "deliver()\n"
        "assert faulthandler.unregister(signal.SIGUSR1)\n"
        "assert not faulthandler.unregister(signal.SIGUSR1)\n"
        "deliver()\n"
        "assert calls == [signal.SIGUSR1, signal.SIGUSR1]\n"
    )
    user = subprocess.run([imre, "-I", source], cwd=root, env=env, capture_output=True, timeout=600)
    assert user.returncode == 0 and not user.stdout, user
    assert user.stderr.count(b"Stack (most recent call first):") == 1, user.stderr
    assert f'File "{source}", line 4 in deliver\n'.encode() in user.stderr, user.stderr


def check_forked_fault_frames(imre, root, run):
    fixture = Path(__file__).with_name("fixtures") / "fork_fault_frames.c"
    probe = root / "fork_fault_frames"
    subprocess.run(
        ["cc", fixture, imre.with_name("libimre_runtime.a"), "-pthread", "-Wl,--wrap=realloc", "-o", probe],
        check=True,
    )
    result = run(probe)
    assert (result.returncode, result.stdout, result.stderr) == (
        0,
        b'Stack (most recent call first):\n  File "child.py", line 30 in child\n'
        b'Stack (most recent call first):\n  File "parent.py", line 20 in inner\n'
        b'  File "parent.py", line 10 in outer\n',
        b"",
    ), result

    fixture = fixture.with_name("fork_fault_registration.c")
    probe = root / "fork_fault_registration"
    subprocess.run(
        ["cc", fixture, imre.with_name("libimre_runtime.a"), "-pthread", "-Wl,--wrap=sigaction", "-o", probe],
        check=True,
    )
    result = run(probe)
    assert (result.returncode, result.stdout, result.stderr) == (0, b"", b""), result


def check_pending_signals(imre, root, env):
    import signal

    fixture = Path(__file__).resolve().with_name("fixtures") / "boxed_signal_pending.py"
    native = root / "signal-pending"
    subprocess.run([imre, "build", fixture, "-o", native], check=True, env=env)
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
        streams = [pickle.dumps((obj, obj.label, bytearray(b"\0\xff")), protocol) for protocol in range(6)]
        entry.write_text(
            "from boxed_pickle_reduction import consume, produce\n"
            f"consume({streams!r})\nproduce()\n"
        )
        for optimization in (0, 3):
            executable = work / f"pickle-O{optimization}"
            build = run(imre, "build", entry, "--module-root", work,
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
                empty, mutable, alias, reduced = value[10]
                assert type(empty) is bytearray and not empty
                assert type(mutable) is bytearray and mutable == payload and mutable is alias
                assert reduced == ("rebuilt", protocol)
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
    passed = run(imre, "-B", "-m", source.stem, "-v", "Cases.test_pass")
    assert passed.returncode == 0, passed
    assert b"test_pass (__main__.Cases.test_pass) ... ok" in passed.stderr.splitlines(), passed
    failed = run(imre, "-B", "-m", source.stem, "-v", "Cases.test_failure")
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
    built = run(imre, "build", source, "--module-root", root, "--no-cache", "-o", executable)
    assert built.returncode == 0, built
    for args, output in [((), b"a\nb\n"), (("Cases.test_b", "Cases.test_a"), b"b\na\n"),
                         (("-vk", "test_b", "Cases"), b"b\n"),
                         (("-vbk", "test_b", "Cases"), b""), (("--buf", "Cases.test_a"), b"")]:
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


def check_unittest_module_main(imre, root, run):
    package = root / "cli_test_package"
    package.mkdir()
    (package / "__init__.py").touch()
    (package / "sample.py").write_text(
        "import unittest\n"
        "import os, sys\n"
        "from fractions import Fraction\n"
        "class Passing(unittest.TestCase):\n"
        "    def test_one(self):\n"
        "        self.assertEqual(sys.argv[0], os.path.basename(sys.executable) + ' -m unittest')\n"
        "        self.assertEqual((Fraction('6/8') + Fraction(1, 6)).as_integer_ratio(), (11, 12))\n"
        "        self.assertRaises(ZeroDivisionError, Fraction, '1/0')\n"
        "        from xml.etree.ElementTree import Element\n"
        "        self.assertEqual(Element('sample').tag, 'sample')\n"
        "        print(self.id())\n"
        "class Failing(unittest.TestCase):\n"
        "    def test_one(self):\n"
        "        failure_reason = 'deliberate failure'\n"
        "        self.fail(failure_reason)\n"
    )
    declared = {"IMREFLAGS": f"--include-module {package.name}.sample"}
    passing = "cli_test_package.sample.Passing"
    ok = run(imre, "-B", "--include-module", f"{package.name}.sample", "-m", "unittest", "-v", passing, passing + ".test_one")
    assert ok.returncode == 0 and ok.stdout == (passing + ".test_one\n").encode() * 2, ok
    assert ok.stderr.count(f"test_one ({passing}.test_one) ... ok".encode()) == 2, ok
    failing = "cli_test_package.sample.Failing.test_one"
    errors = run(imre, "-m", "unittest", "-v", failing, passing + ".test_missing", extra=declared)
    assert errors.returncode == 1 and b"Ran 2 tests" in errors.stderr, errors
    assert f"FAIL: test_one ({failing})".encode() in errors.stderr, errors
    assert b"ERROR: test_missing (unittest.loader._FailedTest.test_missing)" in errors.stderr, errors
    assert b"FAILED (failures=1, errors=1)" in errors.stderr, errors
    local_line = b"    failure_reason = 'deliberate failure'"
    assert local_line not in errors.stderr.splitlines(), errors
    with_locals = run(imre, "-m", "unittest", "--locals", "-v", failing, passing + ".test_missing", extra=declared)
    assert with_locals.returncode == 1 and b"FAILED (failures=1, errors=1)" in with_locals.stderr, with_locals
    assert local_line in with_locals.stderr.splitlines(), with_locals

    (package / "other.py").write_text("raise AssertionError('outside discovery pattern')\n")
    discovered = run(imre, "-m", "unittest", "discover", "-v", "-s", package.name, "-t", ".",
                     "-p", "sample.py", "-k", "Passing.test_one", extra=declared)
    assert discovered.returncode == 0 and discovered.stdout == (passing + ".test_one\n").encode(), discovered
    assert f"test_one ({passing}.test_one) ... ok".encode() in discovered.stderr.splitlines(), discovered
    assert b"Ran 1 test in " in discovered.stderr, discovered


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
        built = run(imre, "build", imported_fixture(root, entry), "--no-cache",
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

        check_installed_package_data(imre, root, run)
        check_module_search_root(imre, root, run)
        check_registered_entry_operand(imre, root, run)
        check_tracemalloc_startup(imre, run)
        check_registered_run_path(imre, root, run)
        check_frozen_modules_startup(imre, root, run)
        if sys.platform == "darwin" or sys.platform.startswith("linux"):
            check_installed_site(imre, root, run)
            check_faulthandler(imre, root, env)
            check_python_process_identity(imre, root, env)
            check_user_site(imre, root, run)
            check_script_sitecustomize(imre, root, run)
            check_explicit_site_directory(imre, root, run)
        if sys.platform.startswith("linux"):
            check_forked_fault_frames(imre, root, run)
            check_os_abort(imre, run)
            check_os_execvpe(imre, root, run)
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

        marker = root / "retired-flag-ran"
        valid = root / "retired_flag.py"
        valid.write_text(f"open({str(marker)!r}, 'w').close()\n")
        image = root / "retired-flag-image"
        for command, extra in (("check", []), ("build", ["-o", image]), ("run", [])):
            refused = run(imre, command, valid, "--runtime-imports", *extra)
            assert refused.returncode != 0 and refused.stdout == b"", refused
            assert b"Usage: imre check" in refused.stderr, refused.stderr
            assert not marker.exists() and not image.exists()

        entry = root / "serve.py"
        entry.write_text("from application import make_endpoint\nprint(__name__, make_endpoint('ready')())\n")
        (root / "application.py").write_text(
            "def make_endpoint(message):\n"
            "    def endpoint():\n        return message\n"
            "    return endpoint\n"
        )
        reference = run(sys.executable, "-B", entry)
        assert (reference.returncode, reference.stdout) == (0, b"__main__ ready\n"), reference.stderr
        # The module launcher executes this closed local package with its Python closure.
        launched = run(imre, "-B", "-m", entry.stem)
        assert (launched.returncode, launched.stdout) == (0, reference.stdout), launched.stderr
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
        launched = run(imre, "-B", "-m", spec_entry.stem)
        assert (launched.returncode, launched.stdout) == (0, b"pkg memory []\n"), launched.stderr

        (root / "reader.py").write_text(
            "for _ in range(3):\n    print(repr(input()))\n"
            "try:\n    input()\nexcept EOFError:\n    print('EOF')\n"
        )
        for exe in (sys.executable, imre):
            actual = run(exe, "-B", "-c", "import reader", stdin="one\r\n\nλ".encode())
            assert actual.returncode == 0 and not actual.stderr, actual.stderr
            assert actual.stdout == "'one\\r'\n''\n'λ'\nEOF\n".encode()

        check_command_stdin(imre, run)

        payload = b"first\x00" * 4096
        members = gzip.compress(payload, mtime=0) + gzip.compress(b"last\xff", mtime=0)
        result = run(imre, "-m", "gzip", "-d", stdin=members)
        assert result.returncode == 0 and result.stderr == b"", result.stderr
        assert result.stdout == payload + b"last\xff"

        # Native warning emission can need linecache before the program imports anything.
        source = root / "buffering_warning"
        source.write_text('with open(__file__, "rb", buffering=1) as stream: pass\n')
        expected = run(sys.executable, "-Wd", source)
        actual = run(imre, "-Wd", source, timeout=300)
        assert actual.returncode == expected.returncode == 0
        assert expected.stderr.endswith(b"  " + source.read_bytes())
        assert actual.stderr == expected.stderr, (expected.stderr, actual.stderr)

        # The Python CLI reports each unclosed file once during interpreter cleanup.
        resource = root / "resource_warning.py"
        resource.write_text(
            "with open(__file__, 'rb') as closed: closed.read(1)\n"
            "f = open(__file__, 'r+b')\n"
        )
        for args in (("-Wd", "-c", f"f = open({str(resource)!r})"), ("-Wd", resource)):
            expected = run(sys.executable, *args)
            actual = run(imre, *args, timeout=300)
            assert expected.returncode == actual.returncode == 0
            assert expected.stderr.startswith(b"<sys>:0: ResourceWarning: unclosed file ")
            assert actual.stderr == expected.stderr, (expected.stderr, actual.stderr)

        provider = Path(__file__).with_name("fixtures") / "boxed_io_open_warning.py"
        for exe in (sys.executable, imre):
            proc = run(exe, "-Wd", provider, timeout=300)
            assert proc.returncode == 0, proc.stderr
            assert proc.stderr == b"", proc.stderr

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
        actual = run(imre, "-B", "-m", streams.stem)
        assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
        assert actual.stdout == expected.stdout == "stdout π\nraw:then:text\nAoldB\\xe9C".encode()
        assert actual.stderr == expected.stderr == "stderr 🌍\nrestored\nbinary stderr\n".encode()

        shutdown = Path(__file__).with_name("fixtures") / "boxed_stdio_shutdown.py"
        for mode, code in (("broken-pipe", 120), ("detached", 120), ("rewrapped", 0)):
            reference = run(sys.executable, "-B", shutdown, mode)
            result = run(imre, "-B", "-m", shutdown.stem, mode, extra={"PYTHONPATH": str(shutdown.parent)})
            assert result.returncode == reference.returncode == code, result.stderr
            assert (result.stdout, result.stderr) == (reference.stdout, reference.stderr)

        text_close = run(imre, "-B", "-c", "import streams", "close-text")
        assert text_close.returncode == 0, text_close.stderr
        assert (text_close.stdout, text_close.stderr) == (expected.stdout, expected.stderr)

        command = (
            "import warnings\n"
            "def warn():\n"
            "    label = 'moved'; warnings.warn(label, DeprecationWarning)\n"
            "warn()\n"
        )
        dynamic = (
            "import sys; sys.path.append('.')\n"
            "namespace = {}\n"
            "exec(\"def unrelated():\\n    raise ValueError('dynamic')\", namespace)\n"
            "namespace['unrelated']()\n"
        )
        form_feed = "import warnings\n# ignored\fmiddle\nwarnings.warn('failed')"
        for source in (command, dynamic, form_feed):
            expected = run(sys.executable, "-Werror", "-c", source)
            actual = run(imre, "-Werror", "-c", source)
            assert expected.returncode == actual.returncode == 1
            assert actual.stderr == expected.stderr, (expected.stderr, actual.stderr)

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
        actual = run(imre, "-B", "-m", chained.stem)
        assert expected.returncode == actual.returncode == 1
        assert actual.stderr == expected.stderr, (actual.stderr, expected.stderr)

        string_io = root / "string_io.py"
        string_io.write_bytes((Path(__file__).with_name("fixtures") / "boxed_string_io.py").read_bytes())
        expected = run(sys.executable, "-B", string_io)
        actual = run(imre, "run", string_io, "--quiet")
        assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
        assert actual.stdout == expected.stdout and actual.stderr == expected.stderr == b""

        for fixture in ("boxed_exit_stack.py", "boxed_async_exit_stack.py", "boxed_pydoc.py", "io_dict_descriptor.py"):
            script = Path(__file__).with_name("fixtures") / fixture
            expected = run(sys.executable, "-B", script)
            actual = run(imre, "-B", "-m", script.stem, extra={"PYTHONPATH": str(script.parent)})
            assert expected.returncode == actual.returncode == 0, (expected.stderr, actual.stderr)
            assert actual.stdout == expected.stdout and actual.stderr == expected.stderr == b""

        async_scope = root / "async_scope.py"
        for imports, factory in (
            ("from contextlib import nullcontext", "nullcontext"),
            ("from contextlib import nullcontext as scope", "scope"),
            ("import contextlib as ctx", "ctx.nullcontext"),
        ):
            async_scope.write_text(
                f"{imports}\n"
                "async def value() -> int:\n"
                f"    async with {factory}(enter_result=23) as entered:\n"
                "        return entered\n"
                "try:\n"
                "    value().send(None)\n"
                "except StopIteration as finished:\n"
                "    print(finished.value)\n"
            )
            actual = run(imre, "run", async_scope, "--quiet")
            assert (actual.returncode, actual.stdout, actual.stderr) == (0, b"23\n", b""), actual.stderr

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

        def compare_module(source):
            compare("-B", "-m", source.stem, extra={"PYTHONPATH": str(source.parent)})

        late_future = root / "late_future.py"
        for prefix, line in (("import __future__\n", 2),
                             ("def f():\n    class C[T](x): pass\n    global x\n", 4)):
            late_future.write_text(prefix + "from __future__ import annotations\n")
            rejected = run(imre, "check", late_future)
            assert rejected.returncode == 1, rejected
            assert f"late_future.py:{line}:1".encode() in rejected.stderr, rejected

        future_caller = root / "future_caller.py"
        future_caller.write_text(
            "print('before')\ntry:\n    import future_dependency\n"
            "except SyntaxError as exc:\n    print(exc.lineno, exc.offset)\n"
            "with open('future_dependency.py', 'w') as f:\n    f.write('value = 37\\n')\n"
            "try:\n    import future_dependency\n    print(future_dependency.value)\n"
            "except SyntaxError:\n    print('stale source')\n"
        )
        for exe in (sys.executable, imre):
            (root / "future_dependency.py").write_text(
                "print('dependency ran')\nfrom __future__ import annotations\n"
            )
            retried = run(exe, "-B", future_caller)
            assert (retried.returncode, retried.stdout, retried.stderr) == (0, b"before\n2 1\n37\n", b""), retried

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
        compare("-Bm", "module_pkg.child", "-m", "tail", "--include-module", "untouched")
        compare("-B", "-mmodule_pkg", "tail")

        json_input = root / "input.json"
        json_input.write_text('{"items": [1, 2]}')
        for module in ("json", "json.tool"):
            compare("-B", "-m", module, "--compact", json_input)

        binary_input = root / "input.bin"
        binary_input.write_bytes(b"a\xffb\n" * 15)
        compare("-B", "-m", "base64", "-e", binary_input, native_timeout=600)
        binary_input.write_bytes(b"Yf9iCg==\n")
        compare("-B", "-m", "base64", "-d", binary_input, native_timeout=600)

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

        compare("-IB", "-c", r"""import zipfile
with zipfile.ZipFile('stored.zip', 'w') as archive:
    archive.writestr('wheel/data', b'\x00\xff')
with zipfile.ZipFile('stored.zip') as archive:
    print(archive.namelist(), archive.read('wheel/data'))""")

        compressor = Path(__file__).with_name("fixtures") / "zlib_compressobj.py"
        for opt in ("0", "3"):
            compare("-IB", compressor, native_args=("run", compressor, "--quiet", "--opt-level", opt))

        written = run(imre, "-IB", "-c", """import zipfile
with zipfile.ZipFile('zip-writer-capability.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
    archive.writestr('compressed', b'data')""")
        assert (written.returncode, written.stdout, written.stderr) == (0, b"", b""), written.stderr
        with zipfile.ZipFile(root / "zip-writer-capability.zip") as archive:
            assert archive.getinfo("compressed").compress_type == zipfile.ZIP_DEFLATED
            assert archive.read("compressed") == b"data"

        archive = root / "application"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as app:
            for prefix in ("", "sub/"):
                app.writestr(prefix + "__main__.py", (entry / "__main__.py").read_text())
                app.writestr(prefix + "sibling.py", f"marker = {prefix!r}\n")
        malformed = root / "malformed.py"
        for source, line, diagnostic in (
            (b"# header\r\n\r\ns='\xff'\n", 3, b"Non-UTF-8 code starting with '\\xff'"),
            (b"# comment\r# coding: utf-8\rs='\xe9x'\r", 3,
             b"(unicode error) 'utf-8' codec can't decode byte 0xe9 in position 3"),
            (b"# ok\r\n# coding: utf-8 \xff\n", 2,
             b"(unicode error) 'utf-8' codec can't decode byte 0xff in position 21"),
        ):
            malformed.write_bytes(source)
            for exe in (sys.executable, imre):
                failed = run(exe, "-B", malformed)
                assert (failed.returncode, failed.stdout) == (1, b""), failed
                assert f'File "{malformed}", line {line}'.encode() in failed.stderr, failed
                assert b"SyntaxError: " + diagnostic in failed.stderr, failed

        # A .pyc operand is never Python source, even when its bytes form a valid script.
        bytecode = root / "not_source.pyc"
        bytecode.write_text("print('executed as source')\n")
        compare(bytecode, status=1, stderr=b"RuntimeError: Bad magic number in .pyc file\n")
        bytecode.write_bytes(b"")
        compare(bytecode, status=1, stderr=b"EOFError: EOF read where not expected\n")
        if sys.version_info[:2] == (3, 14):
            compare("-I", "-c", "import warnings; [print(f) for f in warnings._get_filters()]",
                    native_timeout=600)
            renamed = root / "bad_bytecode"
            renamed.write_bytes(importlib.util.MAGIC_NUMBER[:2] + b"xx")
            compare(renamed, status=1, stderr=b"RuntimeError: Bad magic number in .pyc file\n")

        compare("application", "tail")
        compare("-I", "./application/sub", "-h")

        site = root / "build-site"
        site.mkdir()
        dependency = site / "installed_price.py"
        dependency.write_text("def price(n: int) -> int: return n * 7\n")
        caller = root / "invoice.py"
        caller.write_text("from installed_price import price\nprint(price(9))\n")
        invoice = root / "invoice"
        built = run(imre, "build", caller, "-o", invoice, extra={"PYTHONPATH": str(site)})
        assert built.returncode == 0, built.stderr
        dependency.unlink()
        caller.unlink()
        result = run(invoice)
        assert (result.returncode, result.stdout, result.stderr) == (0, b"63\n", b""), result

        namespace_package = root / "namespace_plugin"
        namespace_package.mkdir()
        (namespace_package / "helper.py").write_text("globals()['answer'] = 42\n")
        (namespace_package / "entry.py").write_text("from .helper import answer\nprint(answer)\n")
        compare("-B", "-m", "namespace_plugin.entry")

        # Both namespace portions supply the same module and its relative dependency.
        roots = [root / "first_portion", root / "second_portion"]
        for portion in roots:
            package = portion / "backends"
            package.mkdir(parents=True)
            (package / "adapter.py").write_text("from .settings import label\n")
            (package / "settings.py").write_text(f"label = {portion.name!r}\n")
        chooser = root / "choose_backend.py"
        chooser.write_text(
            "import sys\n"
            "sys.path.insert(0, sys.argv[1])\n"
            "from backends.adapter import label\n"
            "print(label)\n"
        )
        search = {"PYTHONPATH": os.pathsep.join(map(str, roots))}
        for portion in roots:
            compare("-B", "-m", "choose_backend", str(portion), extra=search)

        (roots[1] / "backends" / "adapter.py").write_text("from . import optional\n")
        (roots[1] / "backends" / "optional.py").write_text("invalid syntax!\n")
        compare("-B", "-m", "choose_backend", str(roots[0]), extra=search)

        bad = roots[1] / "backends" / "adapter.py"
        bad.write_bytes(b"# coding: latin-1\r\n\xe9 = )\r\n")
        cache = root / "backend-cache"
        saved = root / "choose-backend"
        built = run(imre, "build", chooser, "-o", saved, "--opt-level", "0",
                    extra={**search, "IMRE_CACHE_DIR": str(cache)}, timeout=300)
        assert built.returncode == 0, built.stderr
        chooser.unlink()
        for portion in roots:
            for source in portion.rglob("*.py"):
                source.unlink()
        if cache.exists():
            shutil.rmtree(cache)
        first = run(saved, str(roots[0]), extra=search)
        assert (first.returncode, first.stdout, first.stderr) == (0, b"first_portion\n", b""), first
        second = run(saved, str(roots[1]), extra=search)
        assert (second.returncode, second.stdout) == (1, b""), second
        assert b"SyntaxError" in second.stderr and os.fsencode(bad) in second.stderr, second

        # An ordinary module under the first root wins over a package in a later root.
        (root / "preferred.py").write_text("print('file wins')\n")
        (root / "preferred").mkdir()
        (root / "preferred" / "__main__.py").write_text("invalid syntax!\n")
        later = root / "later" / "preferred"
        later.mkdir(parents=True)
        (later / "__init__.py").write_text("invalid syntax!\n")
        compare("-B", "-m", "preferred", extra={"PYTHONPATH": str(later.parent)})

        # All three entry points must search the same module suppliers in order.
        mailapp = root / "mailapp.py"
        mail_command = "import quopri\nprint(quopri.encodestring(b'A=B'))\n"
        mail_module = mail_command + "print(__spec__.origin)\n"
        mailapp.write_text(mail_command)
        codec = "def encodestring(data: bytes) -> bytes:\n    return %r + data\n"
        with zipfile.ZipFile(archive, "a") as app:
            app.writestr("quopri.py", codec % b"archive:")
            app.writestr("mailcli.py", mail_module)
        (later.parent / "quopri.py").write_text(codec % b"directory:")
        (later.parent / "mailcli.py").write_text(mail_module)
        for roots in ((archive, archive, later.parent), (later.parent, archive)):
            for args in ((mailapp,), ("-c", mail_command), ("-m", "mailcli")):
                compare("-B", *args, extra={"PYTHONPATH": os.pathsep.join(map(str, roots))},
                        native_timeout=300)

        # The same module has a native body at both ZIP origins. The active finder
        # selects it after sys.path changes, without initializing the dormant module.
        archives = [root / "first.whl", root / "second.whl"]
        for index, wheel in enumerate(archives):
            with zipfile.ZipFile(wheel, "w") as app:
                app.writestr("choices/__init__.py", "")
                app.writestr("choices/pick.py", f"def pick(data: bytes): return {index}, bytes(data)\n")
                app.writestr("choices/dormant.py", "raise AssertionError('initialized')\n")
                app.writestr("docs/sample.py", b"print('documentation example')\n")
        selector = root / "choose.py"
        selector.write_text(
            "import sys\nfrom importlib import import_module\n"
            "first, second, module = sys.argv[1:]\n"
            "a, b = sys.path.index(first), sys.path.index(second)\n"
            "sys.path[a], sys.path[b] = sys.path[b], sys.path[a]\n"
            "chosen = import_module(module)\n"
            "print(chosen.pick(bytearray(b'zip')), chosen.__spec__.origin, chosen.__loader__.get_data(chosen.__file__))\n"
            "print(chosen.__loader__.get_data(chosen.__loader__.archive + '/docs/sample.py'))\n"
            "assert 'choices.dormant' not in sys.modules\n"
        )
        compare("-B", selector, *archives, "choices.pick",
                extra={"PYTHONPATH": os.pathsep.join(map(str, archives))}, native_timeout=300)
        ir = root / "choose.ll"
        built = run(imre, "build", selector, "--extension-path", archives[0],
                    "--extension-path", archives[1], "--emit-llvm", ir, "-o", root / "choose", timeout=300)
        assert built.returncode == 0, built.stderr
        with ir.open() as stream:
            llvm = [line for line in stream if ".whl/" in line or "call void @imre_dyn_register_zip_source(" in line]
        registrations = [line for line in llvm if "call void @imre_dyn_register_zip_source(" in line]
        for wheel in archives:
            assert all(str(wheel / "docs/sample.py") not in line for line in llvm)
            for name in ("choices/pick.py", "choices/dormant.py"):
                path = str(wheel / name)
                origin = next(line.split()[0] for line in llvm if f'c"{path}\\00"' in line)
                assert any(f", ptr {origin}, ptr @" in line for line in registrations), path

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

        # Python normalizes the script small L when it parses an identifier.
        logging_command = (
            "name = 'worker'\n"
            "import ℓogging as log\n"
            "record = log.makeLogRecord({'name': name, 'msg': 'items %d', 'args': (3,)})\n"
            "print(log.Formatter('%(name)s %(message)s').format(record))\n"
        )
        compare("-I", "-c", logging_command, native_timeout=600)

        shadow_dir = root / "logging_provider"
        shadow_dir.mkdir()
        (shadow_dir / "logging.py").write_text("marker = 'project logging'\n")
        shadow_script = shadow_dir / "entry.py"
        shadow_script.write_text(
            "import sys\nsys.path.append('unused')\nimport logging\nprint(logging.marker)\n"
        )
        compare("-B", shadow_script, extra={"IMREFLAGS": "--opt-level 0"}, native_timeout=300)

        (root / "shell_words.py").write_text(
            "import shlex\n"
            "words = ['', 'two words', '$HOME']\n"
            "print(shlex.split(shlex.join(words)))\n"
            "print(list(shlex.shlex('a && b', posix=True, punctuation_chars=True)))\n"
        )
        compare("-B", "-m", "shell_words")

        (root / "merge_counts.py").write_text(
            "from trace import CoverageResults\n"
            "counts = CoverageResults({('app.py', 3): 2})\n"
            "counts.update(CoverageResults({('app.py', 3): 4, ('app.py', 5): 1}))\n"
            "print(sorted(counts.counts.items()))\n"
        )
        compare("-B", "-m", "merge_counts")

        tokenize_script = root / "tokenize_entry.py"
        tokenize_script.write_text(
            "from io import StringIO\n"
            "from tokenize import generate_tokens\n"
            "for t in generate_tokens(StringIO('x += 1 # note').readline):\n"
            "    print(t.string, t.exact_type)\n"
        )
        compare("-B", tokenize_script)

        pyio_script = root / "pyio_entry.py"
        pyio_script.write_text(
            "import io, _pyio\n"
            "assert io.BytesIO is not _pyio.BytesIO\n"
            "for stream in (io.BytesIO, _pyio.BytesIO):\n"
            "    with stream(b'abc') as buf:\n"
            "        print(stream.__module__, buf.read())\n"
        )
        compare("-I", pyio_script, native_timeout=600)
        compare(
            "-I", "-c", "import sys; print(__import__(sys.argv[1]).BytesIO(b'abc').read())",
            "_pyio", native_timeout=600,
        )

        pyio_project = root / "pyio_project"
        pyio_project.mkdir()
        (pyio_project / "_pyio.py").write_text("marker = 'local provider'\n")
        local_script = pyio_project / "entry.py"
        local_script.write_text("import _pyio\nprint(_pyio.marker)\n")
        compare("-B", local_script)

        config_script = root / "config_entry.py"
        config_script.write_text(
            "from configparser import ConfigParser\n"
            "config = ConfigParser()\n"
            "config.read_string('[DEFAULT]\\nroot=/opt\\n[app]\\npath=%(root)s/bin')\n"
            "print(config['app']['path'])\n"
        )
        config_env = {"IMREFLAGS": "--opt-level 0"}
        compare("-B", config_script, extra=config_env, native_timeout=600)
        compare(
            "-B", "-c", "import sys; print(__import__(sys.argv[1]).ConfigParser.__name__)",
            "configparser", extra=config_env, native_timeout=600,
        )

        ftp_script = root / "ftp_entry.py"
        ftp_script.write_text("import ftplib\nprint(ftplib.FTP().encoding, ftplib.parse257('257 \"home\"'))\n")
        compare("-B", ftp_script)

        director_script = root / "opener_director.py"
        director_script.write_text(
            "from urllib.request import OpenerDirector, build_opener\n"
            "assert type(build_opener()) is OpenerDirector\n"
        )
        compare("-IB", director_script, native_args=("run", director_script, "--quiet"))

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

        descriptor = root / "namedtuple_descriptor.py"
        descriptor.write_text(
            "from collections import namedtuple\n"
            "Point = namedtuple('Point', 'x y')\n"
            "field = Point.y\n"
            "assert field is Point.y and field.__get__(None, Point) is field\n"
            "print(field.__get__(Point(11, 22)), field.__get__((31, 32)))\n"
        )
        compare("-B", descriptor, native_args=("run", descriptor, "--quiet"))

        (root / "namedtuple_spec.py").write_text(
            "from importlib.machinery import ModuleSpec\n"
            "def make_spec(): return ModuleSpec('example', None)\n"
        )
        options = root / "namedtuple_options.py"
        options.write_text(
            "from collections import namedtuple\n"
            "T = namedtuple('T', ['line', 'class'], defaults=[None], rename=True, module='other')\n"
            "def spec_name():\n"
            "    from namedtuple_spec import make_spec\n"
            "    return make_spec().name\n"
            "print(T(7), T._field_defaults, T.__module__, spec_name())\n"
        )
        compare("-B", options, native_args=("run", options, "--quiet"))

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
            "from locale import getlocale, getpreferredencoding\n"
            "setlocale(LC_ALL, 'C')\n"
            "print(getlocale(), getlocale(category=LC_ALL))\n"
            "print(getpreferredencoding(False), getpreferredencoding(do_setlocale=True))\n"
            "assert setlocale(LC_ALL) == 'C'\n"
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

        accessors = Path(__file__).resolve().parent / "fixtures" / "tempfile_accessors.py"
        compare("-IB", accessors, native_args=("run", accessors, "--quiet"))
        if sys.platform == "linux":
            compare("-B", accessors, extra={"LC_ALL": "C", "PYTHONUTF8": "0", "PYTHONCOERCECLOCALE": "0"})

        if sys.platform == "linux" and sys.version_info >= (3, 13):
            count = Path(__file__).resolve().parent / "fixtures" / "os_cpu_count.py"
            compare("-IB", count)
            compare("-B", count, extra={"PYTHON_CPU_COUNT": "7"})
            compare("-B", "-X", "cpu_count=2", "-X", "cpu_count=3", count,
                    extra={"PYTHON_CPU_COUNT": "7"})
            compare("-E", "-B", "-X", "cpu_count=default", count,
                    extra={"PYTHON_CPU_COUNT": "invalid"})
            for options, extra in (
                (("-B", "-X", "cpu_count=2"), {"PYTHON_CPU_COUNT": "invalid"}),
                (("-IB", "-X", "cpu_count"), {}),
                (("-B",), {"PYTHON_CPU_COUNT": "bad", "PYTHONHASHSEED": "bad"}),
                (("-B",), {"PYTHON_CPU_COUNT": "bad", "PYTHONINTMAXSTRDIGITS": "bad"}),
                (("-B",), {"PYTHON_CPU_COUNT": "bad", "PYTHON_CONTEXT_AWARE_WARNINGS": "bad"}),
                (("-B",), {"PYTHON_CPU_COUNT": "2", "PYTHONHASHSEED": "bad", "PYTHON_CONTEXT_AWARE_WARNINGS": "bad"}),
                (("-B",), {"PYTHON_CPU_COUNT": "2", "PYTHONINTMAXSTRDIGITS": "bad", "PYTHON_CONTEXT_AWARE_WARNINGS": "bad"}),
            ):
                expected = run(sys.executable, *options, "-c", "pass", extra=extra)
                actual = run(imre, *options, "-c", "pass", extra=extra)
                assert (actual.returncode, actual.stdout, actual.stderr) == (1, b"", expected.stderr)
                assert expected.returncode == 1

        names = root / "calendar_type_names.py"
        names.write_text(
            "from datetime import date, datetime, timedelta\n"
            "print(date.__module__, datetime.__qualname__, timedelta.__module__)\n"
        )
        compare("-IB", names, native_args=("run", names, "--quiet"))

        namespace = root / "datetime_class_namespace.py"
        for source in (
            "from datetime import date\nprint(date.__dict__['min'] is date.min)",
            "from datetime import time as T\nprint(vars(T)['max'] is T.max)",
            "import datetime as dt\ncls = dt.timedelta\nprint(vars(cls)['resolution'] is cls.resolution)",
        ):
            namespace.write_text(source + "\n")
            compare("-IB", namespace, native_args=("run", namespace, "--quiet"))

        date_isoformat = root / "date_isoformat.py"
        for body in (
            "print(d.isoformat())",
            "print(date.isoformat(d))",
            "render = d.isoformat\nprint(render())",
            "print(getattr(d, 'isoformat')())",
            "print(getattr(d, 'isoformat', None)())",
        ):
            date_isoformat.write_text("from datetime import date\nd = date(2, 3, 2)\n" + body + "\n")
            compare("-IB", date_isoformat, native_args=("run", date_isoformat, "--quiet"))

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

        for statement, error in (
            ("del (0.0).imag", "AttributeError"),
            ("float.real = 1", "TypeError"),
            ("del float.imag", "TypeError"),
        ):
            compare("-c", f"try:\n    {statement}\nexcept {error} as exc:\n    print(exc)")

        compare("-c", "print(divmod(1.0, 0.1), divmod(0, -2.0), divmod(-2.0, 1))")

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

        compare("-c", "print(2.0 * (3 + 4j), (3 + 4j) * 2.0, 2.0 / (3 + 4j), (3 + 4j) / 2.0)")
        compare("-c", "z = complex(3, 4)\nprint(2.0 * z, z * 2.0, 2.0 / z, z / 2.0)")

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
            "case.addTypeEqualityFunc(int, 'assertLess')\ncase.assertEqual(1, 2)",
            "case.assertTupleEqual((1,), (1,))\n"
            "try: case.assertTupleEqual((1,), [1])\nexcept AssertionError as exc: print(str(exc))",
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
            compare("-B", "-m", array_import.stem)

        zoneinfo_entry = root / "zoneinfo_entry.py"
        (root / "zoneinfo_helper.py").write_text("import zoneinfo as provider\n")
        for statement in (
            "from zoneinfo_helper import provider",
            "import sys\nprovider = __import__(sys.argv[1])",
        ):
            zoneinfo_entry.write_text(
                statement + "\nprint(provider.ZoneInfo.__name__, provider.TZPATH)\n"
                "provider.reset_tzpath([])\nprint(provider.TZPATH)\n"
            )
            compare(
                "-B", zoneinfo_entry, "zoneinfo", extra={"PYTHONTZPATH": str(root)},
                native_args=("run", zoneinfo_entry, "--quiet", "--", "zoneinfo"),
                native_timeout=300,
            )

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
        compare(
            "-IB", "-c",
            "import sys\n"
            "module = __import__(sys.argv[1])\n"
            "print(module.PycInvalidationMode.CHECKED_HASH.name)\n"
            "try:\n    module.compile('missing.py', '.', doraise=True)\n"
            "except FileExistsError as error:\n    print(str(error))",
            "py_compile", native_timeout=300,
        )
        compare(
            "-IB", "-c",
            'from tomllib import loads; print(loads("[tool]\\nport = 8000")["tool"]["port"])',
            native_timeout=300,
        )
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

        compare(
            "-IB", "-c",
            "from copy import copy, deepcopy\n"
            "from functools import lru_cache\n"
            "class C:\n"
            "    @lru_cache()\n"
            "    def f(self): pass\n"
            "wrapped = C.f\n"
            "assert copy(wrapped) is deepcopy(wrapped) is wrapped\n"
            "assert wrapped.__reduce__() == 'C.f'\n"
            "for owner in (type(wrapped), wrapped):\n"
            "    assert owner.__reduce__.__text_signature__ == '($self, /)'\n"
            "    assert owner.__copy__.__text_signature__ is owner.__deepcopy__.__text_signature__ is None\n",
            native_timeout=300,
        )
        getters = Path(__file__).with_name("fixtures") / "typed_attrgetter"
        compare("-IB", getters / "fields.py")
        compare("-B", getters / "main.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_attrgetter.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_itemgetter.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_methodcaller.py")
        symbolic_defaults = Path(__file__).with_name("fixtures") / "inspect_symbolic_defaults.py"
        compare("-IB", symbolic_defaults)
        compare("-IB", "-O", symbolic_defaults)
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_inspect_signature.py")
        (root / "weather_report.py").write_text("def report(temp):\n    return f'{temp} C'\n")
        compare("-Bm", "inspect", "weather_report:report")
        (root / "weather_metadata.py").write_text("import weather_report\nprint(weather_report.__spec__.cached)\n")
        metadata = run(imre, "-Bm", "weather_metadata")
        details = run(imre, "-Bm", "inspect", "--details", "weather_report:report")
        assert metadata.returncode == details.returncode == 0, (metadata.stderr, details.stderr)
        assert details.stdout.splitlines()[2:4] == [b"Cached: " + metadata.stdout.rstrip(b"\n"), b"Line: 0"]
        (root / "station_cache.py").write_text("__cached__ = 'project cache'\ndef report(): pass\n")
        compare("-Bm", "inspect", "--details", "station_cache:report")
        compare("-Bm", "inspect", "sys", status=1, stderr=b"Can't get info for builtin modules.\n")
        local_inspect = root / "inspect.py"
        local_inspect.write_text("origin = 'local inspect'\nif __name__ == '__main__': print(origin)\n")
        try:
            compare("-Bm", "inspect", "sys")
            compare("-B", "-c", "import inspect; print(inspect.origin)")
            (root / "local_inspect_api.py").write_text(
                "import inspect\nprint(inspect.origin, hasattr(inspect, '_signature_layout'))\n"
            )
            compare("-Bm", "local_inspect_api")
        finally:
            local_inspect.unlink()
        static_fixture = Path(__file__).with_name("fixtures") / "boxed_getattr_static.py"
        compare_module(static_fixture)
        inspect_fixture = Path(__file__).with_name("fixtures") / "boxed_inspect_coroutine.py"
        (root / "view_inspection.py").write_bytes(inspect_fixture.read_bytes())
        inspect_entry = root / "inspect_views.py"
        inspect_entry.write_text("import view_inspection\n")
        compare_module(inspect_entry)
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_proxy_operators.py")
        compare("-IB", "-c", "from operator import __getitem__; print(__getitem__((11,), 0))")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_update_wrapper.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_tracebacks.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "inspect_trace.py")
        compare("-IB", Path(__file__).with_name("fixtures") / "boxed_traceback_links.py")
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
        compare("-ISSc", "import sys\nprint(sys.flags.no_site)\nprint(sys.flags.no_user_site)\n")
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
            "class Owner:\n"
            "    def __del__(self): print('finalized')\n"
            "owner = Owner()",
            native_timeout=300,
        )
        compare(
            "-B", "-c",
            "import atexit, os\n"
            "atexit.register(lambda: print('after shutdown', handler.stream is None))\n"
            "import logging\n"
            "handler = logging.FileHandler(os.devnull)\n"
            "atexit.register(lambda: print('before shutdown', handler.stream is None))\n",
            extra={"IMREFLAGS": "--opt-level 0"},
            native_timeout=300,
        )
        compare(
            "-I", "-c",
            "for enabled in (False, True):\n"
            "    if enabled:\n"
            "        def pop(items): return items.pop()\n"
            "    try: print(pop([42]))\n"
            "    except NameError: print('unbound')\n",
        )
        compare("-B", "-c", "print(list.__len__([1, 2]), tuple.__len__((3,)))")
        compare("-B", "-c", "print(str.__hash__('zone') == hash('zone'))")
        compare("-B", "-c", "slot = str.__hash__; print(slot('zone') == hash('zone'))")
        compare(
            "-B", "-c",
            "class Key(str):\n"
            "    __hash__ = str.__hash__\n"
            "print({Key('zone'): 1}['zone'])",
        )
        compare(
            "-B", "-c",
            "def make_key():\n"
            "    class Key(str):\n"
            "        __hash__ = str.__hash__\n"
            "    return Key('zone')\n"
            "print({'zone': 1}[make_key()])",
        )
        if os.name == "posix":
            for base in ("list", "tuple"):
                compare(
                    "-B", "-c",
                    "import os\n"
                    f"class Args({base}):\n"
                    f"    def __len__(self): return {base}.__len__(self)\n"
                    f"    def __getitem__(self, index): return {base}.__getitem__(self, index)\n"
                    "os.execv('/bin/sh', Args(['sh', '-c', 'echo replaced']))\n",
                )
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
            compare("-IB", immediate_exit, status=37,
                    native_args=("run", immediate_exit, "--quiet", "--opt-level", "0"),
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
        compare("-B", "-c", "print('before'); import time as t; t.sleep(0); "
                             "from sys import stdout as out; out.write('after\\n')", native_timeout=180)
        missing = "print('before'); import no_such_ordered_module; print('after')"
        for exe in (sys.executable, imre):
            result = run(exe, "-B", "-c", missing, timeout=180)
            assert result.returncode == 1 and result.stdout == b"before\n", result
            assert b"ModuleNotFoundError" in result.stderr and b"no_such_ordered_module" in result.stderr
        # Keep source-backed imports and invalid commands on their existing diagnostic path.
        (root / "invalid_neighbor.py").write_text("print('inside'); return\n")
        for code in (
            "print('before'); import invalid_neighbor", "import invalid_neighbor; print('before'); import sys",
            "print('before'); from __future__ import annotations", "print('before'); lazy import sys",
            "print('before'); return; import sys", "print('before'); import sys; return",
        ):
            result = run(imre, "-B", "-c", code)
            assert result.returncode != 0 and not result.stdout, result
        for flags, extra in (("-B", {"PYTHONSAFEPATH": "1"}), ("-EPB", {}), ("-IB", {})):
            result = run(imre, flags, "-c", "print('before'); import neighbor", extra=extra, timeout=180)
            assert result.returncode == 1 and result.stdout == b"before\n" and b"neighbor" in result.stderr, result
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
            compare("-B", "ascii-alias/main.py")

            # The loader and the import error must agree on the physical directory.
            (byte_dir / "local_owner.py").write_text("value = 41\n")
            probe = (
                "import importlib, os, sys\n"
                "owner = importlib.import_module('local_owner')\n"
                "origin = owner.__spec__.origin\n"
                "print(os.fsencode(sys.path[0]), os.fsencode(origin), origin == owner.__file__)\n"
                "class Spec:\n    has_location = True\n    origin = origin\n"
                "    @property\n    def _initializing(self): raise RuntimeError('unexpected flag lookup')\n"
                "def missing(name):\n"
                "    if name == 'leaf': owner.__name__, owner.__spec__ = 'json', Spec()\n"
                "    raise AttributeError(name)\n"
                "owner.__getattr__ = missing\n"
                "try:\n    exec('from local_owner import leaf')\n"
                "except ImportError as exc:\n    print(owner.value, exc.path is origin, exc.name_from)\n"
            )
            (byte_dir / "main.py").write_text(probe)
            compare("-B", "ascii-alias/main.py")
            compare("-B", "-c", "import os\nos.chdir('ascii-alias')\n" + probe)

            (byte_dir / "trace_owner.py").write_text("def fail(): raise ValueError('traceback sentinel')\n")
            (byte_dir / "main.py").write_text(
                "import importlib, os, traceback\n"
                "owner = importlib.import_module('trace_owner')\n"
                "try:\n    owner.fail()\n"
                "except ValueError as exc: print(owner.__file__ in ''.join(traceback.format_exception(exc)))\n"
                "filename = os.fsdecode(b'bad-\\xfe.py')\n"
                "print(filename in str(SyntaxError('bad', (filename, 2, 1, ''))))\n"
            )
            compare("-B", "ascii-alias/main.py", native_timeout=180)
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
        built = run(imre, "build", source, "-o", image)
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

        invoice = Path(__file__).resolve().with_name("fixtures") / "decimal_invoice.py"
        # Cold optimized builds include the CPython decimal implementation.
        compare("-IB", invoice, native_args=("run", invoice, "--quiet"), native_timeout=300)
        compare(
            "-IB", "-c",
            "import importlib, sys\nsys.path.insert(0, sys.argv[1])\nimportlib.import_module(sys.argv[2])",
            invoice.parent, invoice.stem, native_timeout=300,
        )
        compare(
            "-IB", "-c",
            "import importlib, sys\nD = importlib.import_module(sys.argv[1]).Decimal\n"
            "print((D('-4.225') * D('3.5')).quantize(D('0.01')))",
            "decimal", native_timeout=300,
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
        comparisons = Path(__file__).with_name("fixtures") / "text" / "decimal_comparison.py"
        (helpers / "decimal_comparison.py").write_bytes(comparisons.read_bytes())
        compare("-IB", "-c", decimal_import + "import decimal_comparison")
        (helpers / "decimal_reflected.py").write_text(
            "from decimal import Decimal\n"
            "class Peer:\n"
            "    def __eq__(self, other): return 'reflected equality'\n"
            "    def __gt__(self, other): return 'reflected ordering'\n"
            "value = Decimal('1')\n"
            "print(value == Peer(), value < Peer())\n"
        )
        compare("-IB", "-c", decimal_import + "import decimal_reflected")
        for expression in ("Decimal('1.00') < Decimal(1)", "Decimal('1.00') == 1.0"):
            (helpers / "decimal_boundary.py").write_text(
                f"from decimal import Decimal\nprint({expression})\n"
            )
            compare("-IB", "-c", decimal_import + "import decimal_boundary")

        (helpers / "decimal.py").write_text("class Decimal:\n    def __init__(self, value): self.value = value\n")
        compare("-IB", "-c", decimal_import + "from decimal import Decimal\nprint(Decimal(False).value is False)")

        token_limit = Path(__file__).with_name("fixtures") / "json_integer_token_limit.py"
        for limit in ("640", "0"):
            digit_env = {"PYTHONINTMAXSTRDIGITS": limit}
            compare("-B", token_limit, extra=digit_env)
            compare("-B", token_limit, extra=digit_env, native_args=("run", token_limit, "--quiet"))

        compare(
            "-B", "-c",
            "import uuid\n"
            "from uuid import NAMESPACE_DNS, uuid3\n"
            "print(uuid3(NAMESPACE_DNS, 'python.org'))\n"
            "space = uuid.UUID(int=42)\n"
            "print(uuid.uuid5(space, 'café'), uuid.uuid5(space, b'\\xff'))\n",
            extra={"IMREFLAGS": "--opt-level 0"},
            native_timeout=300,
        )
        if sys.version_info >= (3, 14):
            compare(
                "-B", "-c",
                "from uuid import uuid1\n"
                "from uuid import uuid6\n"
                "from uuid import uuid7\n"
                "from uuid import uuid8\n"
                "print(uuid1(1, 2).version)\n"
                "print(uuid6(1, 2).version)\n"
                "print(uuid7().version)\n"
                "print(uuid8(1, 2, 3).version)\n",
                native_timeout=300,
            )
        (helpers / "uuid").mkdir()
        uuid_import = f"import sys\nsys.path.insert(0, {str(helpers)!r})\nimport uuid\n"
        compare("-IB", "-c", uuid_import + "print(hasattr(uuid, '__path__'), str(uuid.UUID(int=0)))")
        (helpers / "uuid.py").write_text("marker = 'explicit search path'\n")
        compare("-IB", "-c", uuid_import + "print(uuid.marker)")
        compare(
            "-B", "-c",
            "import importlib, os, sys\n"
            "name = sys.argv[1]\n"
            "module = importlib.import_module(name)\n"
            "value = module.uuid4()\n"
            "assert value.version == 4 and value.variant == module.RFC_4122\n"
            "assert value.bytes == value.int.to_bytes(16, 'big')\n"
            "def entropy(n):\n"
            "    assert n == 16\n"
            "    return bytes.fromhex('0123456789abbdef4123456789abcdef')\n"
            "original = os.urandom\n"
            "os.urandom = entropy\n"
            "try:\n"
            "    assert str(module.uuid4()) == '01234567-89ab-4def-8123-456789abcdef'\n"
            "finally:\n"
            "    os.urandom = original\n"
            "del sys.modules[name]\n"
            f"sys.path.insert(0, {str(helpers)!r})\n"
            "print(importlib.import_module(name).marker)\n",
            "uuid",
            extra={"IMREFLAGS": "--opt-level 0"},
            native_timeout=300,
        )

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
        (root / "sign_parameters.py").write_bytes(
            (fixtures / "floats" / "sign_parameters.py").read_bytes()
        )
        compare("-B", "-m", "sign_parameters")
        (root / "scalar_returns.py").write_bytes(
            (fixtures / "boxed_scalar_return_bindings.py").read_bytes()
        )
        result = run(imre, "-B", "-m", "scalar_returns")
        assert result.returncode == 0 and not result.stderr, result

        scalar_parameters = root / "scalar_parameters.py"
        scalar_parameters.write_text(
            (fixtures / "registered_reduction_callback.py").read_text()
            + """
def invoke(function, value):
    try:
        function(value, value, send)
    except TypeError as error:
        print(error)

publish.__annotations__["a"] = str
print(publish.__annotations__["a"] is str)
invoke(publish, 3)
invoke(publish, "bad")

invoke(publish, True)
"""
        )
        for args in [(scalar_parameters,), ("-m", "scalar_parameters")]:
            result = run(imre, "-B", *args, stdin=b"first\n3\n4\n12\n")
            assert result.returncode == 0 and not result.stderr, result
            assert result.stdout == b'first 7\n8\nTrue\nfirst 6\npublish argument "a" requires int\nfirst 2\n'

        served = root / "served"
        served.mkdir()
        (served / "a&b").touch()
        (root / "http_files.py").write_bytes((fixtures / "boxed_http_files.py").read_bytes())
        compare("-B", "-m", "http_files", "http.server", str(root),
                extra={"IMRE_ENCODED_FLAGS": "--opt-level\x1f0"}, native_timeout=180)

        compare(
            "-B", "-c",
            "import sys, unittest\n"
            "assert 'unittest.mock' not in sys.modules\n"
            "mock = __import__(sys.argv[1], fromlist=['Mock'])\n"
            "print(mock is unittest.mock, mock.Mock(return_value=42)())",
            "unittest.mock", extra={"IMRE_ENCODED_FLAGS": "--opt-level\x1f0"},
        )

        compare("-B", "-c", "from typing import Protocol as P; print(P.__name__)")
        compare("-B", fixtures / "typing_protocol_import.py")
        compare("-B", fixtures / "boxed_conditional_class_type_alias.py",
                extra={"IMREFLAGS": "--opt-level 0"}, native_timeout=300)
        compare("-B", fixtures / "boxed_list_finalization.py")
        compare(
            "-IB", "-c",
            "from datetime import datetime, timezone\n"
            "d = datetime(2000, 1, 2, 3, tzinfo=timezone.utc)\n"
            "print(d.astimezone(), d.astimezone(tz=None))",
            extra={"TZ": "EST5"},
        )
        element_user = root / "element_user.py"
        shutil.copyfile(fixtures / "boxed_xml_etree.py", root / "element_helper.py")
        element_user.write_text("import element_helper\n")
        compare("-B", element_user, native_args=("run", element_user, "--quiet"), native_timeout=300)
        compare(
            "-B", "-c", "import importlib, sys\nmodule = importlib.import_module(sys.argv[1])\nprint(module.__name__)",
            "xml.etree.ElementTree", native_timeout=300,
        )
        # A computed import must not register ElementTree in the caller's native image.
        registry = run(
            imre, "-B", "-c",
            "import sys\n"
            "from _imre_importlib import registered_path_spec\n"
            "assert __import__(sys.argv[1]).__name__ == sys.argv[1]\n"
            "root = '<imre stdlib>'\n"
            "assert registered_path_spec('importlib', root) is not None\n"
            "for name in sys.argv[2:]:\n"
            "    path = '/'.join([root] + name.split('.')[:-1])\n"
            "    assert registered_path_spec(name, path) is None, name",
            "sys", "xml", "xml.etree", "xml.etree.ElementTree", "xml.etree.ElementPath", timeout=300,
        )
        assert (registry.returncode, registry.stdout, registry.stderr) == (0, b"", b""), registry
        if Path("/usr/share/zoneinfo/America/New_York").is_file():
            compare("-IB", fixtures / "boxed_zoneinfo.py", native_timeout=300)
        compare("-IB", fixtures / "boxed_pyexpat.py", native_timeout=300)
        compare("-B", fixtures / "html_parser_feed.py", native_timeout=300)
        compare("-IB", fixtures / "boxed_pickle_writer.py")
        for first in ("_pickle", "pickle"):
            compare("-IB", fixtures / "pickle_accelerator_writer.py", first, native_timeout=600)
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
        compare("-IB", final_fixture, native_args=("run", final_fixture, "--quiet"))
        # Check each call in its own module so one object input cannot mask another.
        ini = "[formatters]\nkeys=\n[handlers]\nkeys=\n[loggers]\nkeys=root\n[logger_root]\nlevel=INFO\nhandlers=\n"
        config_path = root / "logging.ini"
        config_path.write_text(ini.replace("INFO", "%(threshold)s"))
        config_call = root / "file_config_call.py"
        for setup, call in (
            ("from io import StringIO", f"configure(fname=StringIO({ini!r}))"),
            ("", f"configure(defaults={{'threshold': 'ERROR', 'retries': 3}}, fname={str(config_path)!r})"),
            ("options = {'defaults': {'threshold': 'WARNING'}}",
             f"configure({str(config_path)!r}, **options)"),
        ):
            config_call.write_text(
                "from logging.config import fileConfig as configure\nfrom logging import getLogger\n"
                f"{setup}\nresult = {call}\nassert result is None\nprint(getLogger().level)\n"
            )
            compare("-IB", config_call, native_args=("run", config_call, "--quiet"), native_timeout=600)
        config_call.write_text(
            "from io import StringIO\nfrom logging.config import fileConfig\n"
            "def configure() -> None:\n"
            f"    result = fileConfig(StringIO({ini!r}))\n    assert result is None\n"
            "    level: int = 'wrong'\n    print(level)\nconfigure()\n"
        )
        bad_config = run(imre, "check", config_call)
        assert bad_config.returncode != 0 and b"expected int" in bad_config.stderr, bad_config.stderr
        config_call = root / "dict_config_call.py"
        config_call.write_text(
            "import logging\nfrom logging.config import dictConfig\n"
            "dictConfig({'version': 1, 'incremental': False, 'handlers': {'out': {'class': 'logging.StreamHandler', "
            "'stream': 'ext://sys.stdout'}}, 'root': {'level': 'INFO', 'handlers': ['out']}})\n"
            "logging.info('configured handler')\n"
            "dictConfig({'version': 1, 'root': {'handlers': []}})\n"
            "assert logging.getHandlerByName('out') is None\n"
        )
        compare("-IB", config_call, native_args=("run", config_call, "--quiet"), native_timeout=600)
        config_call.write_text(
            "from logging.config import dictConfig\n"
            "def configure() -> None:\n"
            "    result = dictConfig({'version': 1, 'handlers': {'out': {'class': 'logging.NullHandler'}}})\n"
            "    assert result is None\n"
            "    level: int = 'wrong'\n    print(level)\nconfigure()\n"
        )
        bad_config = run(imre, "check", config_call)
        assert bad_config.returncode != 0 and b"expected int" in bad_config.stderr, bad_config.stderr
        io_registration = fixtures / "io_abc_registration.py"
        compare("-IB", io_registration, native_args=("run", io_registration, "--quiet"))
        override_fixture = fixtures / "boxed_typing_override.py"
        compare("-IB", override_fixture, native_args=("run", override_fixture, "--quiet"))
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
        compare(
            "-IB", "-c",
            "import atexit, sys, weakref\n"
            "class Observer:\n"
            "    def __del__(self):\n"
            "        print('finalizer', sys.is_finalizing(), kept is self, ref() is None)\n"
            "kept = Observer()\n"
            "ref = weakref.ref(kept)\n"
            "atexit.register(lambda: print('atexit', sys.is_finalizing(), ref() is kept))",
        )
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
        syslog = fixtures / "logging_syslog_datagram.py"
        compare("-IB", syslog, native_args=("run", syslog, "--quiet"), native_timeout=600)
        if sys.platform in ("linux", "darwin") and importlib.util.find_spec("_ssl"):
            import ctypes
            try:
                ctypes.CDLL("libssl.3.dylib" if sys.platform == "darwin" else "libssl.so.3")
            except OSError:
                pass
            else:
                stats = fixtures / "ssl_session_stats.py"
                compare("-IB", stats, native_args=("run", stats, "--quiet"), native_timeout=600)
                wrappers = fixtures / "ssl_context_wrappers.py"
                compare("-IB", wrappers, native_args=("run", wrappers, "--quiet"), native_timeout=600)
                ciphers = fixtures / "ssl_context_ciphers.py"
                compare("-IB", ciphers, native_args=("run", ciphers, "--quiet"), native_timeout=600)
                bio = fixtures / "ssl_memory_bio.py"
                compare("-IB", bio, native_args=("run", bio, "--quiet"), native_timeout=600)
                hostname = fixtures / "ssl_check_hostname.py"
                compare("-IB", hostname, native_args=("run", hostname, "--quiet"), native_timeout=600)
                flags = fixtures / "ssl_verify_flags.py"
                compare("-IB", flags, native_args=("run", flags, "--quiet"), native_timeout=600)
                certdata = fixtures.parents[1] / "benchmarks/cpython_stdlib/full/shared/upstream/Lib/test/certdata"
                alpn = fixtures / "ssl_alpn.py"
                compare(
                    "-IB", alpn, certdata,
                    native_args=("run", alpn, "--quiet", "--", certdata), native_timeout=600,
                )
                sni = fixtures / "ssl_sni.py"
                compare(
                    "-IB", sni, certdata,
                    native_args=("run", sni, "--quiet", "--", certdata), native_timeout=600,
                )
                dh_params = fixtures / "ssl_dh_params.py"
                compare(
                    "-IB", dh_params, certdata,
                    native_args=("run", dh_params, "--quiet", "--", certdata), native_timeout=600,
                )
                handshake = fixtures / "ssl_socket_handshake.py"
                compare(
                    "-IB", handshake, certdata,
                    native_args=("run", handshake, "--quiet", "--", certdata), native_timeout=600,
                )
                failed_shutdown = fixtures / "ssl_socket_failed_shutdown.py"
                guarded = run(imre, "run", failed_shutdown, "--quiet", "--", certdata, timeout=600)
                assert guarded.returncode == 0, guarded.stderr
                object_io = fixtures / "ssl_object_io.py"
                compare(
                    "-IB", object_io, certdata,
                    native_args=("run", object_io, "--quiet", "--", certdata), native_timeout=600,
                )
                cert_chain = fixtures / "ssl_cert_chain.py"
                compare(
                    "-IB", cert_chain, certdata,
                    native_args=("run", cert_chain, "--quiet", "--", certdata), native_timeout=600,
                )
                unwrap = fixtures / "ssl_object_unwrap.py"
                compare(
                    "-IB", unwrap, certdata,
                    native_args=("run", unwrap, "--quiet", "--", certdata), native_timeout=600,
                )
                locations = fixtures / "ssl_verify_locations.py"
                compare(
                    "-IB", locations, certdata,
                    native_args=("run", locations, "--quiet", "--", certdata), native_timeout=600,
                )
        queue_exports = fixtures / "logging_queue_exports.py"
        compare("-IB", queue_exports, native_args=("run", queue_exports, "--quiet"), native_timeout=600)
        thread_hook_exports = root / "thread_hook_exports.py"
        thread_hook_exports.write_text(
            "from threading import ExceptHookArgs, __excepthook__, excepthook\n"
            "print(ExceptHookArgs((None, None, None, None)).thread is None, excepthook is __excepthook__)\n"
        )
        compare("-IB", thread_hook_exports, native_args=("run", thread_hook_exports, "--quiet"), native_timeout=300)
        plugin.write_bytes((fixtures / "boxed_threading_except_hook.py").read_bytes())
        compare("-IB", "-c", imports + "arithmetic.current()", native_timeout=300)
        compare(
            "-B", "-c",
            "import importlib, sys\n"
            "handlers = importlib.import_module(sys.argv[1])\n"
            "logging = sys.modules['logging']\n"
            "assert handlers is logging.handlers\n"
            "handler = handlers.BufferingHandler(2)\n"
            "handler.handle(logging.makeLogRecord({'msg': 'late handler'}))\n"
            "print(handler.buffer[0].getMessage())\n"
            "handler.close()\n",
            "logging.handlers", native_timeout=300,
        )
        compare(
            "-B", "-c",
            "import importlib, sys\n"
            "parser = importlib.import_module(sys.argv[1])\n"
            "address, options = parser.get_angle_addr('<ops@example.org> SIZE=19')\n"
            "print(address.addr_spec, options)\n",
            "email._header_value_parser", native_timeout=300,
        )
        compare(
            "-B", "-c",
            "import importlib, sys\n"
            "parser = importlib.import_module(sys.argv[1]).OptionParser()\n"
            "parser.add_option('--file', action='append')\n"
            "options, args = parser.parse_args(['--file=a', '--file', 'b', 'rest'])\n"
            "print(options.file, args, '--file' in parser.format_help())\n",
            "optparse", native_timeout=300,
        )
        compare(
            "-B", "-c",
            "import importlib, sys\n"
            "colors = importlib.import_module(sys.argv[1])\n"
            "print(colors.rgb_to_hls(0.25, 0.5, 0.75))\n",
            "colorsys", native_timeout=300,
        )
        # A from-import elsewhere in the same program would already select the boxed runtime.
        queue_lookup = root / "queue_lookup.py"
        for statement, name in (("import logging.handlers", "logging.handlers"),
                                ("import logging.handlers as h", "h")):
            queue_lookup.write_text(f"{statement}\nprint({name}.QueueListener.__name__)\n")
            checked = run(imre, "check", queue_lookup)
            assert checked.returncode == 0, checked.stderr
        timeout_script = root / "default_timeout.py"
        timeout_script.write_text(
            "from ftplib import FTP\n"
            "from http.client import HTTPConnection\n"
            "from socket import _GLOBAL_DEFAULT_TIMEOUT as default\n"
            "assert FTP().timeout is HTTPConnection('localhost').timeout is default\n"
            "assert FTP(timeout=None).timeout is HTTPConnection('localhost', timeout=None).timeout is None\n"
        )
        compare("-IB", timeout_script, native_args=("run", timeout_script, "--quiet"))
        socket_alias = root / "socket_timeout_alias.py"
        socket_alias.write_text(
            "from _socket import timeout as low_timeout\nfrom socket import timeout\n"
            "try:\n    raise low_timeout('expired')\nexcept timeout as exc:\n    print(str(exc))\n"
        )
        compare("-IB", socket_alias, native_args=("run", socket_alias, "--quiet"))
        socket_alias.write_text("from _socket import timeout\nvalue: int = 'wrong'\nprint(value)\n")
        bad_alias = run(imre, "check", socket_alias)
        assert bad_alias.returncode != 0 and b"expected int" in bad_alias.stderr, bad_alias.stderr
        io_fixture = fixtures / "boxed_io_indirect_import.py"
        compare_module(io_fixture)
        for name in ("boxed_prelinked_coroutine.py", "boxed_coroutine_origin.py"):
            coroutine_fixture = fixtures / name
            compare_module(coroutine_fixture)
        builtins_fixture = fixtures / "boxed_builtins_module.py"
        compare_module(builtins_fixture)
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
            "boxed_threading_condition_event.py", "boxed_threading_timer.py",
            "boxed_thread_captured_lock.py",
            "boxed_thread_bound_lock.py", "boxed_thread_argument_lifetime.py",
            "boxed_thread_captured_acquire.py", "boxed_thread_method_closure.py",
            "boxed_thread_finalization.py",
            "boxed_thread_hash_update.py",
        ):
            thread_fixture = fixtures / name
            compare_module(thread_fixture)
        metaclass_new = fixtures / "ordinary_metaclass_new.py"
        compare("-IB", metaclass_new, native_args=("run", metaclass_new, "--quiet"))
        class_locals = fixtures / "boxed_class_locals.py"
        compare_module(class_locals)
        # An unused generic in the entry must preserve first-party argument diagnostics.
        generic_scope = root / "generic-scope-contract"
        generic_scope.mkdir()
        (generic_scope / "scores.py").write_text(
            "def take(n: int) -> int:\n    return 7\n\nscore = take('x')\n"
        )
        caller = generic_scope / "entry.py"
        caller.write_text(
            "import sys\nfrom scores import score\nprint(score)\n"
            "if False:\n    def unused[T](): pass\n"
        )
        rejected = run(imre, "check", caller)
        assert rejected.returncode == 1, rejected
        assert b"scores.py:4:14: Imre: expected int, got str" in rejected.stderr, rejected

        caller.write_text(
            "def keep(fn): return fn\ndef parent(original):\n"
            "    @keep\n    def show[T: int](value: T = original) -> T: return value\n"
            "    return show\nfn = parent(17)\n"
            "print(fn(), fn.__type_params__[0].__name__)\n"
        )
        compare("-IB", caller, native_args=(caller,))
        for name in ("module_generic_function.py", "postponed_generic_method.py"):
            generic = fixtures / name
            compare("-IB", generic, native_args=("run", generic, "--quiet"))
        compare("-IB", "-c", "import typing; from typing import Text; print(Text is typing.Text is str)")
        reveal = fixtures / "typing_reveal_type.py"
        for command in ((sys.executable, "-IB", reveal),
                        (imre, "run", reveal, "--quiet")):
            result = run(*command)
            assert (result.returncode, result.stdout, result.stderr) == (
                0, b"", b"Runtime type is 'Marker'\nRuntime type is 'type'\n",
            ), result
        introspection = fixtures / "typing_introspection_functions.py"
        compare("-IB", introspection, native_args=("run", introspection, "--quiet"))
        hints_fixture = fixtures / "boxed_typing_get_type_hints.py"
        compare("-IB", hints_fixture, native_args=("run", hints_fixture, "--quiet"))
        if sys.platform == "linux":
            exe = root / "marshal-allocation"
            built = run(imre, "build", imported_fixture(root, fixtures / "boxed_marshal_allocation.py"),
                        "--quiet", "--no-cache", "-o", exe)
            assert built.returncode == 0, built.stderr
            check_marshal_allocation(exe)
        if sys.platform == "linux":
            import resource

            source = root / "nested.py"
            source.write_text("".join(" " * i + "if True:\n" for i in range(96)) + " " * 96 + "result = 7\n")
            executable = root / "source-branches"
            built = run(imre, "build", imported_fixture(root, fixtures / "boxed_source_lower_stack.py"), "-o", executable)
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
            "def binary(value: int) -> str:\n    return bin(value)\n"
            "print(binary(0), binary(-(1 << 150) + 1))",
            "import builtins\nfrom builtins import bin as formatter\n"
            "class Number:\n    def __index__(self): return -33\n"
            "print(formatter is builtins.bin is bin, formatter(Number()), formatter(True))\n"
            "try: formatter(number=1)\nexcept TypeError as error: print(error)",
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
        shadow = root / "hmac_shadow"
        shadow.mkdir()
        (shadow / "hmac.py").write_text("marker = 'local'\n")
        shadow_entry = shadow / "entry.py"
        shadow_entry.write_text(
            "import sys\n"
            "from importlib.machinery import SourceFileLoader\n"
            "from importlib.util import module_from_spec, spec_from_file_location\n"
            "sys.modules['_hashlib'] = sys.modules['_operator'] = None\n"
            "loader = SourceFileLoader('hmac', sys.argv[1])\n"
            "module = module_from_spec(spec_from_file_location('hmac', sys.argv[1], loader=loader))\n"
            "sys.modules['hmac'] = module\n"
            "loader.exec_module(module)\n"
            "assert __import__('hmac') is module and module.marker == 'local'\n"
            "for name in ('digest', '_digest_native', 'compare_digest', 'new', 'HMAC'):\n"
            "    assert not hasattr(module, name)\n"
        )
        local_hmac = shadow / "hmac.py"
        compare(
            "-IB", shadow_entry, local_hmac,
            native_args=("run", shadow_entry, "--module-root", shadow, "--quiet", "--", local_hmac),
        )

        # Only exercise legacy-provider policy when the host OpenSSL can load it.
        policy = root / "hmac-legacy.cnf"
        policy.write_text(
            "openssl_conf = init\n[init]\nproviders = providers\nalg_section = properties\n"
            "[providers]\ndefault = default_provider\nlegacy = legacy_provider\n"
            "[default_provider]\nactivate = 1\n[legacy_provider]\nactivate = 1\n"
            "[properties]\ndefault_properties = provider=legacy\n"
        )
        policy_env = {"OPENSSL_CONF": str(policy)}
        legacy_capabilities = """import _hashlib
for name in ("md4", "ripemd160"):
    print(_hashlib.new(name, b"abc").hexdigest())
for name in ("md4", "ripemd160"):
    try:
        _hashlib.hmac_digest(b"k", b"m", name)
    except ValueError as exc:
        print(type(exc).__name__, str(exc))
    else:
        print("MAC permitted")
"""
        probe = run(sys.executable, "-IB", "-c", legacy_capabilities, extra=policy_env)
        if probe.returncode == 0:
            lines = probe.stdout.splitlines()
            assert lines[:2] == [
                b"a448017aaf21d8525fc10ae87aa6729d",
                b"8eb208f7e05d987a9b044a8e98c6b087f15a0bfc",
            ]
            if lines[2:] == [b"ValueError [digital envelope routines] unsupported"] * 2:
                hmac_entry = fixtures / "bytes/hmac_digest.py"
                compare(
                    "-IB", hmac_entry, "legacy-policy", extra=policy_env,
                    native_args=("run", hmac_entry, "--quiet", "--", "legacy-policy"),
                )

        compare("-B", fixtures / "boxed_unittest_result_registry.py", native_timeout=300)
        compare("-IB", fixtures / "boxed_unittest_named_loader.py")
        compare("-IB", fixtures / "unawaited_coroutine.py")
        async_case = fixtures / "unittest_isolated_asyncio.py"
        compare("-IB", async_case, native_args=("run", async_case, "--quiet"), native_timeout=600)
        compare("-IB", fixtures / "boxed_unittest_patch.py")
        call_args = fixtures / "unittest_mock_call_args.py"
        compare("-IB", call_args, native_args=("run", call_args, "--quiet"))
        boxed_call_args = fixtures / "boxed_unittest_call_args.py"
        compare_module(boxed_call_args)
        compare_module(fixtures / "boxed_doctest_results.py")
        mock_compile_flags = {"IMREFLAGS": "--opt-level 0"}
        patch_multiple = root / "patch_multiple.py"
        for call in ("with patch.multiple", "replace = patch.multiple\nwith replace"):
            patch_multiple.write_text(
                "from unittest.mock import patch\n"
                "class Service:\n    port = 1\n"
                f"{call}(Service, port=2):\n    assert Service.port == 2\n"
                "assert Service.port == 1\n"
            )
            compare("-B", patch_multiple, extra=mock_compile_flags, native_timeout=300)
        compare(
            "-B", "-c",
            "from unittest.mock import Mock\n"
            "parent = Mock()\n"
            "child = Mock()\n"
            "parent.attach_mock(child, 'client')\n"
            "assert parent.client is child\n",
            extra=mock_compile_flags, native_timeout=300,
        )
        compare(
            "-B", "-c", "from unittest.mock import Mock\nm = Mock(['close'])\nm.close()\nassert not hasattr(m, 'read')\n",
            extra=mock_compile_flags, native_timeout=300,
        )
        for name in (
            "boxed_unittest_mock_open.py", "boxed_unittest_autospec_function.py",
            "boxed_unittest_autospec.py", "unittest_mock_callable_spec.py",
        ):
            fixture = fixtures / name
            compare(
                "-IB", fixture, extra=mock_compile_flags,
                native_args=("run", fixture, "--quiet"),
                native_timeout=300,
            )
        compare(
            "-B", "-c",
            "from ast import BinOp, ClassDef\nimport ast\n"
            "assert BinOp is ast.BinOp and ClassDef is ast.ClassDef\n",
        )
        compare("-IB", fixtures / "boxed_unittest_threadingmock.py", extra=mock_compile_flags, native_timeout=300)
        for name in ("boxed_unittest_mock_any.py", "boxed_unittest_mock_side_effect.py", "boxed_unittest_mock_call.py"):
            fixture = fixtures / name
            compare("-IB", fixture, native_args=("run", fixture, "--quiet"))
        method_entry = root / "scalar_method.py"
        for body in (
            "assert b'xy'.rjust(3, b'.') == b'.xy'",
            "pad = bytes.rjust\nassert pad(b'xy', 3, b'.') == b'.xy'",
            "assert ' '.isprintable() and not '\\u00a0'.isprintable()",
            "printable = str.isprintable\n"
            "assert printable('') and printable('A\\u0301')\n"
            "assert not printable('\\n') and not printable('\\u20c1') and not printable(chr(0xd800))",
        ):
            method_entry.write_text(body + "\n")
            compare("-B", method_entry)
        plugin.write_bytes((fixtures / "boxed_zstd.py").read_bytes())
        compare("-B", "-c", imports + "arithmetic.current()", extra={"IMREFLAGS": "--opt-level 0"})
        for fixture in (
            "boxed_ast.py",
            "boxed_ast_compound.py",
            "boxed_ast_encoded_source.py",
            "boxed_ast_visitors.py",
            "boxed_ast_unparse.py",
            "boxed_ast_literal_eval.py",
            "boxed_code_objects.py",
            "boxed_inspect_getmembers.py",
            "boxed_inspect_getmodule.py",
            "boxed_inspect_sourcelines.py",
            "boxed_inspect_stack.py",
            "boxed_inspect_unwrap.py",
            "boxed_inspect_isroutine.py",
            "boxed_inspect_isabstract.py",
            "boxed_numeric_repr.py",
            "boxed_decimal_hash.py",
            "boxed_decimal_arithmetic.py",
            "boxed_recursive_repr.py",
            "boxed_bound_method_repr.py",
            "boxed_scalar_parameter_defaults.py",
            "boxed_positional_builtin_methods.py",
            "boxed_positional_string_methods.py",
            "boxed_simple_namespace.py",
            "boxed_namedtuple_repr.py",
            "boxed_namedtuple_factory.py",
            "boxed_sequence_registration.py",
            "boxed_slice_deep_repr.py",
            "boxed_slice_repr_cycles.py",
            "boxed_source_exec.py",
            "boxed_indentation_limit.py",
            "boxed_source_except.py",
            "boxed_interactive.py",
            "boxed_relative_package_imports.py",
            "boxed_sys_getframe.py",
            "boxed_sys_getsizeof.py",
            "boxed_sys_audit.py",
            "boxed_sys_asyncgen_hooks.py",
            "boxed_sys_audit_recursion.py",
            "boxed_sys_recursion_limit.py",
            "boxed_sys_intern.py",
            "boxed_sys_byteorder.py",
            "boxed_sys_stdlib_module_names.py",
            "boxed_sys_float_info.py",
            "boxed_sys_hash_info.py",
            "boxed_sys_thread_info.py",
            "boxed_sys_flags.py",
            "boxed_sys_jit.py",
            "boxed_sys_monitoring_events.py",
            "boxed_bytes_add.py",
            "boxed_binary_iterators.py",
            "boxed_array.py",
            "boxed_bytearray_memoryview.py",
            "boxed_bytearray_insert.py",
            "boxed_bytearray_pop.py",
            "boxed_fileio_buffer_export.py",
            "boxed_bytes_io.py",
            "boxed_io_newline_decoder.py",
            "boxed_io_utf16.py",
            "boxed_text_io_wrapper.py",
            "boxed_platform_version.py",
            "boxed_int_to_bytes.py",
            "boxed_int_bit_count.py",
            "boxed_int_rational_fields.py",
            "boxed_int_from_bytes.py",
            "boxed_ipaddress.py",
            "boxed_int_conversion.py",
            "boxed_comparison_wrappers.py",
            "boxed_divmod_slots.py",
            "boxed_integer_division_slots.py",
            "boxed_integer_shift_slots.py",
            "boxed_integer_operator_descriptors.py",
            "boxed_complex.py",
            "boxed_float.py",
            "boxed_round.py",
            "boxed_math_rounding.py",
            "boxed_math_acosh.py",
            "boxed_math_asin.py",
            "boxed_math_angles.py",
            "boxed_math_fmod.py",
            "boxed_math_atan.py",
            "boxed_math_comb.py",
            "boxed_math_factorial.py",
            "boxed_math_perm.py",
            "boxed_math_dist.py",
            "boxed_math_fabs.py",
            "boxed_math_fma.py",
            "boxed_math_log1p.py",
            "boxed_math_pow.py",
            "boxed_math_remainder.py",
            "boxed_math_frexp.py",
            "boxed_statistics_correlation.py",
            "boxed_statistics_covariance.py",
            "boxed_statistics_linear_regression.py",
            "boxed_statistics_mean.py",
            "boxed_statistics_harmonic_mean.py",
            "boxed_statistics_decimal_ratio.py",
            "boxed_math_fsum.py",
            "boxed_math_gcd.py",
            "boxed_math_hyperbolic.py",
            "boxed_math_hypot.py",
            "boxed_math_prod.py",
            "boxed_math_sumprod.py",
            "boxed_math_isclose.py",
            "boxed_math_log2.py",
            "boxed_math_ldexp.py",
            "boxed_math_nextafter.py",
            "boxed_float_arithmetic.py",
            "boxed_bytes_fields.py",
            "boxed_binary_subclass_lengths.py",
            "boxed_binary_ljust.py",
            "boxed_binary_strip.py",
            "boxed_bytes_removeprefix.py",
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
            "boxed_operator_identity.py",
            "boxed_random.py",
            "boxed_secrets.py",
            "boxed_pkgutil.py",
            "boxed_lru_cache_equality.py",
            "boxed_lru_cache_metadata.py",
            "boxed_functools_cache.py",
            "boxed_functools_reduce.py",
            "boxed_functools_singledispatch.py",
            "boxed_functools_partialmethod.py",
            "boxed_functools_partial_state.py",
            "boxed_functools_partial_generic_alias.py",
            "boxed_asynccontextmanager_protocol.py",
            "boxed_contextmanager.py",
            "boxed_contextlib_aclosing.py",
            "boxed_total_ordering.py",
            "boxed_cmp_to_key.py",
            "boxed_lru_insert_eviction.py",
            "boxed_lru_insert_same.py",
            "boxed_forward_ref_syntax.py",
            "boxed_annotationlib_refs.py",
            "boxed_class_annotation_sites.py",
            "boxed_future_module_annotations.py",
            "boxed_deferred_module_annotations.py",
            "boxed_typing_no_type_check.py",
            "boxed_typing_no_type_check_decorator.py",
            "boxed_typing_assert_type.py",
            "boxed_typing_exports.py",
            "boxed_typing_any_weakref.py",
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
            "boxed_typing_cache_cleanup.py",
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
            "boxed_template_client.py",
            "boxed_fstring_join.py",
            "boxed_template_literals.py",
            "boxed_re_ascii.py",
            "boxed_re_numeric_escapes.py",
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
            "boxed_memoryview_shape.py",
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
            "boxed_dynamic_class_attribute.py",
            "boxed_make_dataclass.py",
            "boxed_dataclass_slots.py",
            "boxed_dataclass_init_annotations.py",
            "boxed_dataclass_direct_initializer.py",
            "boxed_dataclass_classvars.py",
            "boxed_email_dates.py",
            "boxed_email_addresses.py",
            "boxed_html.py",
            "boxed_wave.py",
            "boxed_http_message.py",
            "boxed_time.py",
            "boxed_timeit.py",
            "boxed_time_timezone.py",
            "boxed_time_strftime.py",
            "time_strptime.py",
            "boxed_codecs.py",
            "boxed_codec_registry.py",
            "boxed_wide_decode.py",
            "boxed_buffered_partial_write.py",
            "boxed_io_exports.py",
            "boxed_io_unsupported_operation.py",
            "boxed_io_reimport.py",
            "boxed_fileio_opener.py",
            "boxed_fileio_write.py",
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
            "boxed_os_exports.py",
            "boxed_os_python_fspath.py",
            "boxed_os_file_descriptors.py",
            "boxed_os_fdopen.py",
            "boxed_binary_bytes_paths.py",
            "boxed_buffered_readlines.py",
            "boxed_os_eventfd.py",
            "boxed_os_timerfd.py",
            "boxed_os_descriptor_io.py",
            "boxed_os_ftruncate.py",
            "boxed_os_lockf.py",
            "boxed_os_mknod.py",
            "boxed_os_pathconf.py",
            "boxed_os_pathconf_aliases.py",
            "boxed_os_pread.py",
            "boxed_os_pwrite.py",
            "boxed_os_readinto.py",
            "boxed_os_waitpid.py",
            "boxed_os_process_signal.py",
            "boxed_os_execv.py",
            "boxed_posix_session.py",
            "boxed_os_stat.py",
            "boxed_os_terminal_size.py",
            "boxed_termios_attributes.py",
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
            "boxed_urllib_cookies.py",
            "boxed_http_transport.py",
            "boxed_message_parameters.py",
            "boxed_email_parser.py",
            "boxed_unicodedata.py",
            "boxed_descriptors.py",
            "boxed_attribute_name_keys.py",
            "boxed_exception_repr.py",
            "boxed_exception_notes.py",
            "boxed_exception_normalization.py",
            "boxed_exception_groups.py",
            "boxed_warning_categories.py",
            "boxed_warnings.py",
            "boxed_warnings_accelerator.py",
            "boxed_tracemalloc.py",
            "boxed_oserror.py",
            "boxed_type_identity.py",
            "boxed_type_namespace.py",
            "boxed_abc_namespace.py",
            "boxed_singleton_types.py",
            "boxed_singleton_subclass.py",
            "boxed_function_closure_cells.py",
            "boxed_function_closure_edges.py",
            "boxed_nested_callable_scope.py",
            "boxed_function_annotation_closure.py",
            "boxed_functiontype.py",
            "boxed_module_function_builtins.py",
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
            "boxed_unittest_assert_dict_equal.py",
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
            "boxed_unittest_format_message.py",
            "boxed_unittest_assert_warns.py",
            "boxed_unittest_assert_logs.py",
            "boxed_unittest_assert_regex.py",
            "boxed_unittest_identity.py",
            "boxed_unittest_skip.py",
            "boxed_unittest_seal.py",
            "boxed_frozenset.py",
            "boxed_attribute_deletion.py",
            "boxed_mixed_delete.py",
            "boxed_delete_eager_key.py",
            "boxed_delete_operand_lifetime.py",
            "boxed_store_operand_lifetime.py",
            "boxed_decorator_operand_lifetime.py",
            "boxed_recursion.py",
            "boxed_function_defaults.py",
            "boxed_reprlib.py",
            "boxed_asyncio_policy.py",
            "boxed_multiprocessing_context.py",
            "boxed_textwrap.py",
            "boxed_set_ordering.py",
            "boxed_set_iteration.py",
            "boxed_dict_view_equality.py",
            "boxed_set_issubset.py",
            "boxed_set_operator_descriptors.py",
            "boxed_set_union.py",
            "boxed_set_difference_update.py",
            "boxed_set_intersection_update.py",
            "boxed_set_symmetric_difference_update.py",
            "boxed_set_difference.py",
            "boxed_set_intersection.py",
            "boxed_starred_call_order.py",
            "boxed_fixed_keyword_calls.py",
            "boxed_tuple_membership.py",
            "boxed_set_intersection_lifetime.py",
            "boxed_partition_field.py",
            "boxed_set_pop.py",
            "boxed_set_clear.py",
            "boxed_set_remove.py",
            "boxed_weakref.py",
            "boxed_public_weakref.py",
            "boxed_weakref_unraisable.py",
            "boxed_gc_collect.py",
            "boxed_gc_controls.py",
            "boxed_gc_finalizers.py",
            "boxed_gc_generator_loop.py",
            "boxed_gc_generator_resurrection.py",
            "boxed_gc_returned_stream.py",
            "boxed_weak_frame_return.py",
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
            "boxed_unittest_result_format.py",
            "boxed_unittest_program_options.py",
            "boxed_unittest_result_buffer.py",
            "boxed_unittest_result_registry.py",
            "boxed_unittest_get_description.py",
            "boxed_unittest_default_result.py",
            "boxed_unittest_return_warning.py",
            "boxed_unittest_runner_warnings.py",
            "boxed_unittest_runner_durations.py",
            "boxed_unittest_base_suite.py",
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
            "boxed_input_callbacks.py",
            "boxed_redirect_streams.py",
            "boxed_with_lookup.py",
            "boxed_itertools_repeat.py",
            "boxed_itertools_type_namespaces.py",
            "boxed_itertools_immutable_types.py",
            "boxed_itertools_cursor_repr.py",
            "boxed_itertools_chain.py",
            "boxed_pairwise.py",
            "boxed_itertools_cycle.py",
            "boxed_itertools_accumulate.py",
            "boxed_itertools_batched.py",
            "boxed_itertools_zip_longest.py",
            "boxed_itertools_tee.py",
            "boxed_itertools_combinations.py",
            "boxed_itertools_permutations.py",
            "boxed_itertools_product.py",
            "boxed_itertools_groupby.py",
            "boxed_itertools_filterfalse.py",
            "boxed_itertools_takewhile.py",
            "boxed_itertools_dropwhile.py",
            "boxed_itertools_compress.py",
            "boxed_starmap_subclass.py",
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
            "boxed_list_iter_descriptor.py",
            "boxed_list_setitem.py",
            "boxed_sequence_repeat.py",
            "boxed_primitive_sequence_literals.py",
            "boxed_sequence_order.py",
            "boxed_tuple_iter_descriptor.py",
            "boxed_tuple_index.py",
            "boxed_tuple_slice_identity.py",
            "boxed_json_containers.py",
            "boxed_json_scanner.py",
            "boxed_json_decoder.py",
            "boxed_json_module_doc.py",
            "boxed_json_encoder.py",
            "boxed_json_encoder_accelerator.py",
            "boxed_json_quote.py",
            "boxed_deque.py",
            "boxed_deque_iterator_reduction.py",
            "boxed_deque_repeat.py",
            "boxed_deque_deep_equal.py",
            "boxed_deque_cycle_callbacks.py",
            "boxed_deque_mixed_equal.py",
            "boxed_deque_nested_mutation.py",
            "boxed_deque_concat.py",
            "boxed_zip.py",
            "boxed_map.py",
            "boxed_python_iterators.py",
            "boxed_filter_reduction.py",
            "boxed_float_sum.py",
            "boxed_complex_sum.py",
            "boxed_enumerate_methods.py",
            "boxed_enumerate_metadata.py",
            "boxed_sorted.py",
            "boxed_max.py",
            "boxed_min.py",
            "boxed_comprehension_outer_yield.py",
            "boxed_generator_delegation.py",
            "boxed_suspended_operand_positions.py",
            "boxed_lambda_default_positions.py",
            "boxed_mixed_lambda_defaults.py",
            "boxed_source_coroutine.py",
            "boxed_coroutine_origin.py",
            "boxed_anext.py",
            "boxed_async_generator.py",
            "boxed_comprehension_plan.py",
            "boxed_comprehension_operands.py",
            "boxed_source_async_generator.py",
            "boxed_source_async_with.py",
            "boxed_with_control.py",
            "boxed_short_circuit.py",
            "boxed_while_truth.py",
            "boxed_positional_class_match.py",
            "boxed_doctest_suite.py",
            "boxed_doctest_hooks.py",
            "boxed_doctest_files.py",
            "boxed_slice.py",
            "boxed_binascii.py",
            "boxed_zlib.py",
            "boxed_gzip_decompress.py",
            "boxed_bz2.py",
            "boxed_lzma.py",
            "boxed_hashlib_names.py",
            "bytes/hashlib_md5_provider.py",
            "bytes/hashlib_sha3_provider.py",
            "bytes/hashlib_kdf.py",
            "bytes/hmac_digest.py",
            "bytes/hmac_digest_fallback.py",
            "boxed_hashlib_file_digest.py",
            "boxed_standard_base64.py",
            "boxed_csv.py",
            "boxed_bytes_count.py",
            "boxed_bytes_separators.py",
            "boxed_binary_splitlines.py",
            "boxed_bytes_join.py",
            "boxed_bytes_fromhex.py",
            "boxed_bytes_hex.py",
            "boxed_bytes_lower.py",
            "boxed_binary_endswith.py",
            "boxed_struct.py",
            "boxed_pickle_writer.py",
            "boxed_reversed_list.py",
            "boxed_reversed_range.py",
            "boxed_heapq.py",
            "boxed_pydoc.py",
            "boxed_queue.py",
            "boxed_shutil.py",
            "boxed_difflib.py",
            "boxed_tokenize.py",
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
            elif fixture in (
                "boxed_nested_callable_scope.py",
                "boxed_mixed_lambda_defaults.py",
                "boxed_starred_call_order.py",
                "boxed_codec_registry.py",
                "boxed_gc_generator_loop.py",
                "boxed_gc_generator_resurrection.py",
                "boxed_gc_returned_stream.py",
                "boxed_weak_frame_return.py",
            ):
                compare("-IB", "-c", imports)
            else:
                compare("-IB", "-c", imports + "arithmetic.current()")
            if fixture == "boxed_generator_delegation.py":
                named = root / fixture
                named.write_text(plugin.read_text() + "\ncurrent()")
                compare("-IB", named, native_args=("run", named, "--quiet"))
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
            raw_environment = {b"RAW_\xff": b"value_\xff", "LC_ALL": "C"}
            compare("-B", "-c", r"import os; print(ascii(os.getenv('RAW_\udcff')))", extra=raw_environment)
            compare("-B", "-c", r"from os import getenvb as read; print(read(b'RAW_\xff'))", extra=raw_environment)
            compare("-B", "-c", "import os; print('PATH' in list(os.environ.keys()))")
            compare("-B", "-c", """from os import environb as env
keys = env.keys()
key = b'IMRE_KEYS_VIEW_\\xff'
env[key] = b'\\xfe'
assert key in keys
del env[key]
assert key not in keys
try:
    env.keys(1)
except TypeError:
    pass
else:
    raise AssertionError("keys accepted an argument")
""")
            check_boxed_unittest_main_reporting(imre, root, run)
            check_unittest_main_names(imre, root, run)
            check_unittest_module_main(imre, root, run)
            check_unittest_file_discovery(imre, root, run)
            plugin.write_bytes((fixtures / "boxed_subprocess_fileio.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_exports.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-EOOB", "-Werror::ResourceWarning", "-X", "utf8=0",
                    "-c", imports + "arithmetic.interpreter_flags()")
            plugin.write_bytes((fixtures / "boxed_subprocess_interruptible_read.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-B", "-c", "from subprocess import call; "
                    "print(call(['/bin/sh', '-c', 'exit 47'], startupinfo=None, creationflags=0.0))")
            plugin.write_bytes((fixtures / "boxed_subprocess_platform_options.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_pass_fds.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-B", "-c", "from subprocess import Popen; "
                    "p = Popen(['/bin/true']); p.wait(); print(repr(p))")
            plugin.write_bytes((fixtures / "boxed_subprocess_repr.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_returncode.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_lifetime.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_buffering.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_subprocess_text.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            script = root / "subprocess_text.py"
            command = r"printf '\200'; printf '\223err\224' >&2; exit 7"
            script.write_text(
                "import subprocess\n"
                f"cmd = ['/bin/sh', '-c', {command!r}]\n"
                "try: subprocess.run(cmd, capture_output=True, check=True, encoding='cp1252')\n"
                "except subprocess.CalledProcessError as exc:\n"
                "    print(exc.output == '€', exc.stderr == '“err”', exc.args == (7, cmd))\n"
            )
            compare("-B", script)

            script.write_text(
                "import subprocess\n"
                "print(subprocess.check_output(['/bin/cat'], input='€', encoding='cp1252'))\n"
            )
            compare("-B", script)

            script.write_text(
                "import subprocess\n"
                "cmd = ['/bin/printf', r'\\200']\n"
                "print(subprocess.check_output(cmd, errors='backslashreplace'))\n"
                "print(ascii(subprocess.run(cmd, capture_output=True, errors='surrogateescape').stdout))\n"
            )
            compare("-B", script)

            capture = (
                "def capture() -> str:\n"
                "    return subprocess.check_output(['/bin/printf', r'\\200'], encoding='cp1252')\n"
            )
            for invalid, diagnostic in (
                ("def invalid() -> int:\n    return 'bad'\n", b"invalid-return-type"),
                ("def needs_int(value: int): pass\ndef invalid():\n    needs_int('invalid')\n", b"invalid-argument-type"),
            ):
                for definitions, calls in (
                    (invalid + capture, "invalid()\nprint(capture())"),
                    (invalid + capture, "print(capture())"),
                    (capture + invalid, "print(capture())\ninvalid()"),
                    (capture + invalid, "print(capture())"),
                ):
                    script.write_text("import subprocess\n" + definitions + calls + "\n")
                    result = run(imre, "-B", script)
                    assert result.returncode > 0 and not result.stdout, result
                    assert diagnostic in result.stderr or b"expected int, got str" in result.stderr, result

            script.write_text(
                "import subprocess\n"
                "def ordinary(value): pass\n"
                "def caught():\n"
                "    try: subprocess.run(['/bin/cat'], input=b'bad', capture_output=True, encoding='cp1252')\n"
                "    except AttributeError: print('input rejected')\n"
                "    try: ordinary()\n"
                "    except TypeError: print('argument rejected')\n"
                "caught()\n"
            )
            compare("-B", script)

            script.write_text(
                "import io\n"
                "def acquire(): return io.BytesIO(b'x').getbuffer()\n"
                "def wrong() -> int: return 'wrong'\n"
                "print(acquire().tobytes())\n"
                "try: wrong()\n"
                "except TypeError: print('return rejected')\n"
            )
            result = run(imre, "-B", script)
            assert result.returncode == 0 and result.stderr == b"", result
            assert result.stdout == b"b'x'\nreturn rejected\n", result

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
        if sys.platform in ("darwin", "linux"):
            compare("-IB", str(fixtures / "boxed_geteuid_exports.py"), "os", "posix")
            plugin.write_bytes((fixtures / "boxed_os_times.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_grp.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_pwd.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_expanduser.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-B", "-c", imports + "arithmetic.current()",
                    extra={"LC_ALL": "C", "PYTHONUTF8": "0", "PYTHONCOERCECLOCALE": "0"})
        if sys.platform == "linux":
            plugin.write_bytes((fixtures / "boxed_os_waitpid.py").read_bytes())
            child = run(imre, "-IB", "-c", imports + "arithmetic.pidfd()")
            assert (child.returncode, child.stdout, child.stderr) == (0, b"", b""), child
            plugin.write_bytes((fixtures / "boxed_os_system_queries.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_os_getrandom.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-IB", str(fixtures / "epoll_descriptors.py"))
            plugin.write_bytes((fixtures / "boxed_mmap_integer.py").read_bytes())
            child = run(imre, "-IB", "-c", imports + "arithmetic.current()")
            assert (child.returncode, child.stdout, child.stderr) == (0, b"", b""), child
            plugin.write_bytes((fixtures / "boxed_mmap.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-IB", "-c", imports + "arithmetic.assignment_callbacks()")
            compare("-IB", "-c", imports + "arithmetic.limits()")
            plugin.write_bytes((fixtures / "boxed_epoll_capability.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current(False)")
            compare("-IB", "-c", imports + "arithmetic.current(True)")
            execve = str(fixtures / "boxed_os_execve.py")
            compare("-IB", execve, "exports", "os")
            for mode in ("path", "fd"):
                compare("-IB", execve, mode, "os", status=17)
            plugin.write_bytes((fixtures / "boxed_os_spawn.py").read_bytes())
            libc, version = platform.libc_ver()
            child_callbacks = libc == "glibc" and tuple(map(int, version.split(".")[:2])) >= (2, 32)
            compare("-IB", "-c", imports + f"arithmetic.current({child_callbacks})")
            if child_callbacks:
                plugin.write_bytes((fixtures / "boxed_os_fork.py").read_bytes())
                compare("-IB", "-c", imports + "arithmetic.current()")
                compare("-IB", "-c", imports + "arithmetic.threaded_spawn()")
                plugin.write_bytes((fixtures / "boxed_subprocess_preexec.py").read_bytes())
                compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_fadvise.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_fallocate.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-IB", str(fixtures / "boxed_posix_spawn_exports.py"), "posix", "os")
            plugin.write_bytes((fixtures / "boxed_posix_spawn_actions.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_sched_param.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_sched_getscheduler.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            compare("-IB", str(fixtures / "boxed_posix_xattr_exports.py"), "os", "posix")
            plugin.write_bytes((fixtures / "boxed_posix_xattrs.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_fcntl.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_rtld_flags.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_confstr.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_sysconf.py").read_bytes())
            compare("-IB", "-c", imports + "arithmetic.current()")
            plugin.write_bytes((fixtures / "boxed_posix_device.py").read_bytes())
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
                build = run(imre, "build", imported_fixture(root, probe), "--opt-level", opt, "-o", binary)
                assert build.returncode == 0, build.stderr
                fault = run(binary, extra=injected)
                assert (fault.returncode, fault.stdout, fault.stderr) == (0, oracle.stdout, b""), fault
                retry = run(binary, extra=injected | {"IMRE_SECRETS_TEST_SOURCE": "reject"})
                assert (retry.returncode, retry.stdout, retry.stderr) == (0, b"c\n", b"draw\ndraw\n"), retry
        # A nested declaration is invalid even if the surrounding function is never called.
        source = root / "generic-nonlocal.py"
        source.write_text("def factory():\n    class Box[T]:\n        def f(self):\n            nonlocal T\n")
        invalid = run(imre, "build", source, "-o", root / "invalid-nonlocal")
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
            built = run(imre, "build", fixture, "-o", executable)
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
            compare("-B", digits, extra=digit_env, native_args=("run", digits, "--quiet"))

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
