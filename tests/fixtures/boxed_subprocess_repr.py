import subprocess


class Child(subprocess.Popen):
    pass


def current():
    args = ["/bin/sh", "-c", "read x; exit 7"]
    with subprocess.Popen(args, stdin=subprocess.PIPE) as child:
        assert repr(child) == "<Popen: returncode: None args: ['/bin/sh', '-c', 'read x; exit 7']>"
        child.stdin.close()
        assert child.wait() == 7
        assert repr(child) == "<Popen: returncode: 7 args: ['/bin/sh', '-c', 'read x; exit 7']>"

    prefix = "<Child: returncode: 0 args: ['"
    suffix = "']>"
    for length in (80, 81):
        arg = "λ" * (length - len(prefix) - len(suffix))
        with Child([arg], executable="/bin/true") as child:
            assert child.wait() == 0
            full = prefix + arg + suffix
            assert repr(child) == (full if length == 80 else full[:76] + "...>")
