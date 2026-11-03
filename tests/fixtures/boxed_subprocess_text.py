import subprocess


def current():
    pipe = subprocess.PIPE
    cmd = ["/bin/sh", "-c", "cat; printf 'err\\r\\n' >&2"]
    for mode in ({"text": True}, {"universal_newlines": 1}):
        with subprocess.Popen(cmd, stdin=pipe, stdout=pipe, stderr=pipe,
                              **mode) as child:
            assert child.communicate("a\rx\r\ny\n") == ("a\nx\ny\n", "err\n")
            assert child.returncode == 0

    with subprocess.Popen(cmd, stdin=pipe, stdout=pipe, stderr=pipe,
                          universal_newlines=False) as child:
        assert child.communicate(b"\xe9\rx\r\ny\n") == (b"\xe9\rx\r\ny\n", b"err\r\n")

    with subprocess.Popen(["/bin/sh", "-c", "cat; printf '\\377\\r' >&2"],
                          stdin=pipe, stdout=pipe, stderr=pipe, encoding="ascii",
                          errors="replace", universal_newlines=False) as child:
        assert child.communicate("é\r") == ("?\n", "\ufffd\n")
        assert child.universal_newlines == child.text_mode == "ascii"

    with subprocess.Popen(["/bin/true"], stdin=pipe, text=2, universal_newlines=1) as child:
        try:
            child.stdin.write(b"x")
        except TypeError:
            pass
        else:
            raise AssertionError("text stdin accepted bytes")

    with subprocess.Popen(["/bin/printf", "xy\\r\\nz"], text=True, stdout=pipe) as child:
        assert child.stdout.read(1) == "x"
        child.universal_newlines = False
        assert child.communicate() == ("y\nz", None)

    try:
        subprocess.Popen(["/no/such/child"], stdout=pipe, encoding="no-such-encoding")
    except LookupError:
        pass
    else:
        raise AssertionError("invalid pipe codec was accepted")

    with subprocess.Popen(["/bin/true"], text=True) as child:
        assert child.communicate("unused") == (None, None)

    # Conflict validation takes precedence over launching the nonexistent executable.
    try:
        subprocess.Popen(["/no/such/child"], text=False, universal_newlines=1)
    except subprocess.SubprocessError as exc:
        assert str(exc) == ("Cannot disambiguate when both text and universal_newlines "
                            "are supplied but different. Pass one or the other.")
    else:
        raise AssertionError("conflicting text options accepted")
    for encoding in ("utf-16", "utf-32-be"):
        with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe, encoding=encoding) as child:
            assert child.communicate("A😀\r\nB") == ("A😀\nB", None)
    cmd = ["cmp", "-", "/dev/null"]
    with subprocess.Popen(cmd, stdin=pipe, stdout=pipe, encoding="utf-16") as child:
        assert child.communicate("") == ("", None)
        assert child.returncode == 0

    class BadInput(str):
        def encode(self, *args):
            return 123

    for value, error in ((BadInput(""), TypeError), (b"", AttributeError)):
        with subprocess.Popen(cmd, stdin=pipe, stdout=pipe, encoding="utf-16") as child:
            try:
                child.communicate(value)
            except error:
                assert child.stdin.closed
            else:
                raise AssertionError("invalid empty input accepted")

    for suffix in ("fin", None):
        with subprocess.Popen(["/bin/cat"], stdin=pipe, stdout=pipe, encoding="latin-1") as child:
            child.stdin.reconfigure(write_through=False)
            child.stdin.write("pré:")
            assert child.communicate(suffix) == ("pré:" + (suffix or ""), None)
            assert child.returncode == 0
    print("Popen text pipes passed")
