"""Route original CPython callbacks through a copytree running in a native child."""
import ast
import os
import select
import subprocess


class Patterns:
    """Carry the original arguments into a native call to shutil.ignore_patterns."""

    def __init__(self, *patterns):
        self.patterns = patterns


COPY_CALLBACK = '''def copy_file(src: str, dst: str) -> None:
    with open(sys.argv[3], "w") as outgoing:
        outgoing.write(repr((src, dst)) + "\\n")
    with open(sys.argv[4]) as incoming:
        if incoming.readline() != "ok\\n":
            raise RuntimeError("original callback raised")
'''

SOURCE = "import shutil\nimport sys\n\n" + COPY_CALLBACK + '''
def ignore_names(src: str, names: list[str]) -> list[str]:
    with open(sys.argv[3], "w") as outgoing:
        outgoing.write(repr((src, names)) + "\\n")
    ignored: list[str] = []
    with open(sys.argv[4]) as incoming:
        count = int(incoming.readline())
        if count < 0:
            raise RuntimeError("original callback raised")
        for i in range(count):
            ignored.append(names[int(incoming.readline())])
    return ignored

try:
    if sys.argv[5] == "copy":
        print(shutil.copytree(sys.argv[1], sys.argv[2], copy_function=copy_file))
    elif sys.argv[5] == "ignore":
        print(shutil.copytree(sys.argv[1], sys.argv[2], ignore=ignore_names))
    else:
        patterns = shutil.ignore_patterns(sys.argv[6], sys.argv[7])
        print(shutil.copytree(sys.argv[1], sys.argv[2], ignore=patterns))
except RuntimeError as exc:
    print(str(exc))
'''


def patterns(binary, src, dst, arguments):
    assert len(arguments) == 2 and all(isinstance(p, str) for p in arguments)
    p = subprocess.run([binary, src, dst, "", "", "patterns", *arguments],
                       capture_output=True, text=True, check=True, timeout=30)
    assert (p.stdout, p.stderr) == (os.fsdecode(dst) + "\n", "")
    return dst


def call(binary, src, dst, callback, events, *, ignore=False, env=None, pass_fds=(), expected_dst=None):
    request, child_write = os.pipe()
    child_read, reply = os.pipe()
    try:
        proc = subprocess.Popen([binary, src, dst, f"/proc/self/fd/{child_write}",
                                 f"/proc/self/fd/{child_read}", "ignore" if ignore else "copy"],
                                pass_fds=(child_write, child_read, *pass_fds), env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    except BaseException:
        os.close(request)
        os.close(reply)
        raise
    finally:
        os.close(child_write)
        os.close(child_read)
    raised = None
    try:
        with os.fdopen(request, "rb") as rx, os.fdopen(reply, "wb", buffering=0) as tx:
            while True:
                if not select.select([rx], [], [], 30)[0]:
                    raise TimeoutError("native copytree stopped responding")
                line = rx.readline()
                if not line:
                    break
                paths = ast.literal_eval(line.decode())
                assert isinstance(paths, tuple) and len(paths) == 2
                if ignore:
                    assert isinstance(paths[0], str) and isinstance(paths[1], list)
                    assert all(type(name) is str for name in paths[1])
                    events.append(["str", "list[str]"])
                else:
                    events.append([type(path).__name__ for path in paths])
                try:
                    result = callback(*paths)
                    if ignore:
                        indexes = [str(i) for i, name in enumerate(paths[1]) if name in result]
                        message = ("\n".join([str(len(indexes)), *indexes]) + "\n").encode()
                    else:
                        message = b"ok\n"
                except BaseException as error:
                    raised = error
                    tx.write(b"-1\n" if ignore else b"raised\n")
                else:
                    tx.write(message)
        stdout, stderr = proc.communicate(timeout=30)
        assert proc.returncode == 0 and not stderr, (proc.returncode, stdout, stderr)
        if raised is not None:
            assert stdout == "original callback raised\n", stdout
            raise raised
        expected = dst if expected_dst is None else expected_dst
        assert stdout == os.fsdecode(expected) + "\n", stdout
        return expected
    finally:
        if proc.poll() is None:
            proc.kill()
        proc.wait()
