"""File and decoder semantics that CPython's 29 original cases do not distinguish."""

import linecache
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def check_files(bridge, root):
    data = [
        b"",
        b"\r",
        b"a\rb\r\nc\nd",
        "a\u0085b\u2028c\u2029d".encode(),
        b"\xef\xbb\xbfjust source\n",
        b"\xef\xbb\xbf# coding:utf-8\n\xc3\xa9",
        b"\xef\xbb\xbf# coding: latin-1\n\xe9",
        b"\xef\xbb\xbf# coding: utf8\n",
        b"#!/usr/bin/env python\n\t# coding=LATIN_1\n\xff",
        b"first\n# coding:latin-1\n\xff",
        b"# coding:unknown-charset\n",
        b"# coding:ascii\n\xff",
        b"# coding:cp1252\n\x80\r\n",
        b"# coding:cp1252\n\x81",
        b"x\0y\n",
        b"# ok\n\x00",
        b"one\ntwo\n\x00three",
        b"# \xff\n# coding=latin-1\n",
        b"# \xff\ntext\n",
        b"#!\n# coding:latin-1-xy\n\x80\n",
        b"# coding: coding=utf-8\n",
        b"coding:utf-8\n",
        b"# coding:\x0cutf-8\n",
        b"# coding:latin-1-abcdefgh\n\xff",
        b"# coding:UTF-8-invalid\n\xc3",
        b"\xef\xbb\xbf\xef\xbb\xbftext",
        b"   \t\f# coding=.latin-1\n\xff",
    ]
    # For each single-byte charmap, exercise every successful byte in one source
    # and every undefined byte separately. A single bad byte must not mask the
    # decoders' valid mappings. The product never launches Python to decode.
    table = (
        HERE.parents[3] / "crates/imre-runtime/src/objects/linecache/codec_names.txt"
    )
    representatives = {}
    for row in table.read_text().splitlines():
        alias, kind = row.split()
        if kind.isdecimal():
            representatives.setdefault(kind, alias)
    for alias in [*representatives.values(), "ascii", "latin1"]:
        good, bad = bytearray(), bytearray()
        for b in range(256):
            try:
                bytes([b]).decode(alias)
            except UnicodeDecodeError:
                bad.append(b)
            else:
                good.append(b)
        header = f"# coding:{alias}\n".encode()
        data.append(header + good)
        data.extend(header + bytes([b]) for b in bad)
    for i, content in enumerate(data):
        filename = str(root / f"codec-{i}.py")
        Path(filename).write_bytes(content)
        # Python, not our generated tables, determines every expected result.
        expected = linecache.getlines(filename)
        actual = bridge.getlines(filename)
        if actual != expected or (filename in bridge.cache) != (
            filename in linecache.cache
        ):
            raise AssertionError((i, content[:45], actual, expected))
    # A recognized non-text transform is distinct from an unknown codec, which
    # yields [] above. Recognized non-native multibyte decoders fail explicitly.
    for alias in ["base64", "hex_codec", "rot13", "shift_jis"]:
        filename = str(root / f"{alias}.py")
        Path(filename).write_bytes(f"# coding:{alias}\n".encode())
        if alias == "shift_jis":
            assert linecache.getlines(filename) == [f"# coding:{alias}\n"]
            try:
                bridge.getlines(filename)
            except NotImplementedError as err:
                assert (
                    str(err)
                    == "linecache: native source decoder for 'shift_jis' is unavailable"
                )
            else:
                raise AssertionError(
                    "unimplemented registered codec was silently accepted"
                )
        else:
            try:
                linecache.getlines(filename)
            except LookupError as expected:
                try:
                    bridge.getlines(filename)
                except LookupError as actual:
                    assert str(actual) == str(expected)
                else:
                    raise AssertionError("non-text transform silently accepted")
            else:
                raise AssertionError("CPython accepted non-text encoding")
        assert filename not in bridge.cache
    name = str(root / "same-size.py")
    path = Path(name)
    path.write_bytes(b"A")
    moment = 1_700_000_000_000_000_000
    os.utime(name, ns=(moment, moment))
    assert bridge.getlines(name) == linecache.getlines(name)
    path.write_bytes(b"B")
    # At this epoch one nanosecond is below the resolution of the st_mtime
    # double. Matching size and float time must keep the old list.
    for stamp, expected in [(moment + 1, "A\n"), (moment + 1_000_000_000, "B\n")]:
        os.utime(name, ns=(stamp, stamp))
        bridge.checkcache(name)
        linecache.checkcache(name)
        assert bridge.getline(name, 1) == linecache.getline(name, 1) == expected
    path.unlink()
    bridge.checkcache()
    linecache.checkcache()
    assert bridge.getlines(name) == linecache.getlines(name) == []
    return len(data)


def check_search(lang, root):
    """The cwd, entry directory and configured source directories have distinct precedence."""
    script, cwd, first, second = [
        root / n for n in ("script", "cwd", "first", "second")
    ]
    for d in (script, cwd, first, second):
        d.mkdir()
    body = b"""import linecache
for name in ["local.py","sibling.py","env.py","two.py","missing.zzz"]:
    print(linecache.getlines(name))
"""
    link = root / "invoked"
    link.mkdir()
    (link / "case.py").symlink_to(script / "entry.py")
    exe = lang.build(body, link)
    for d, values in [
        (cwd, ["local"]),
        (script, ["local", "sibling"]),
        (first, ["sibling", "env"]),
        (second, ["sibling", "env", "two"]),
    ]:
        for name in values:
            (d / (name + ".py")).write_text(d.name + "-" + name)
    for safe in ("", "1"):
        env = {
            **os.environ,
            "PYTHONPATH": os.pathsep.join(map(str, (first, second))),
            "PYTHONSAFEPATH": safe,
        }
        results = [
            subprocess.run(command, cwd=cwd, env=env, capture_output=True, check=True)
            for command in ([sys.executable, link / "case.py"], [exe])
        ]
        assert (
            results[0].stdout == results[1].stdout
            and results[0].stderr == results[1].stderr == b""
        ), (safe, results)
        expected = "first-sibling" if safe else "script-sibling"
        assert (
            expected.encode() in results[1].stdout and b"cwd-local" in results[1].stdout
        )
