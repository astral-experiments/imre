import genericpath
import os
import os.path
import posix
import posixpath
from os import altsep, fspath, linesep, pathsep, sep
from os.path import altsep as path_altsep, pathsep as path_list_sep, sep as path_sep
from posixpath import abspath, ismount, normpath


class Path:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        return self.value


class Text(str):
    def __fspath__(self):
        raise AssertionError("string paths bypass __fspath__")


class Raw(bytes):
    def __fspath__(self):
        raise AssertionError("byte paths bypass __fspath__")


class Descriptor:
    def __get__(self, obj, owner):
        raise RuntimeError("descriptor lookup")


class NoMethod:
    __fspath__ = Descriptor()


class Raises:
    def __fspath__(self):
        raise RuntimeError("method call")


def current():
    assert normpath is os.path.normpath is posix._path_normpath
    assert normpath.__name__ == "_path_normpath" and normpath.__self__ is posix
    for value, expected in (
        ("", "."),
        ("//a/../../b", "//b"),
        ("///../x/", "/x"),
        (Text("../a/./b"), "../a/b"),
        (Raw(b"../\xff/./a/.."), b"../\xff"),
        (b"a\0b/../c", b"c"),
    ):
        result = normpath(path=Path(value))
        assert result == expected and type(result) is type(expected)
    assert os.path.splitroot is posixpath.splitroot is posix._path_splitroot_ex
    for path, expected in (
        (Text("relative"), ("", "", "relative")),
        (Text("///a"), ("", "/", "//a")),
        (Raw(b"//\xff"), (b"", b"//", b"\xff")),
        ("/\udcff\x00", ("", "/", "\udcff\x00")),
    ):
        parts = os.path.splitroot(p=Path(path))
        assert parts == expected and all(type(item) is type(expected[0]) for item in parts)
    assert os.path.commonprefix is posixpath.commonprefix is genericpath.commonprefix
    for paths, expected in (
        ([], ""), (["café", "cafés"], "café"), ([Path(b"/a"), Path(b"/b")], b"/"),
        ([["p", "x"], ["p", "y"]], ["p"]), ([(1, 2), (1, 3)], (1,)),
    ):
        assert os.path.commonprefix(paths) == expected
    assert os.path.getsize is posixpath.getsize is genericpath.getsize
    assert os.path.samestat is posixpath.samestat is genericpath.samestat
    assert os.path.samefile is posixpath.samefile is genericpath.samefile
    assert os.path.sameopenfile is posixpath.sameopenfile is genericpath.sameopenfile
    with open(__file__, "rb") as input_file:
        assert genericpath.getsize(filename=__file__) == len(input_file.read())
        fd = input_file.fileno()
        assert os.path.samestat(s1=os.stat(__file__), s2=os.fstat(fd))
        assert os.path.samefile(f1=__file__, f2=__file__)
        assert os.path.sameopenfile(fp1=fd, fp2=fd)
    missing = __file__ + ".missing"
    assert ismount is posixpath.ismount is os.path.ismount
    assert os.path.ismount(path=Path(b"/"))
    assert not os.path.ismount(__file__)
    assert not os.path.ismount(missing)
    mount_link = "imre-boxed-link-to-root"
    os.symlink("/", mount_link)
    try:
        assert not ismount(mount_link)
        assert ismount(mount_link + "/")
    finally:
        os.unlink(mount_link)
    try:
        os.path.getsize(missing)
    except FileNotFoundError as error:
        assert error.filename == missing
    else:
        raise AssertionError("missing file unexpectedly had a size")
    result = os.stat(__file__)
    for name, expected in (
        ("getmtime", result.st_mtime), ("getatime", result.st_atime), ("getctime", result.st_ctime),
    ):
        function = getattr(genericpath, name)
        assert function is getattr(posixpath, name) is getattr(os.path, name)
        assert function(filename=Path(__file__)) == expected
        try:
            function(missing)
        except FileNotFoundError as error:
            assert error.filename == missing
        else:
            raise AssertionError("missing file unexpectedly had a timestamp")
    assert os.path.exists is posixpath.exists is genericpath.exists
    assert os.path.lexists is posixpath.lexists is genericpath.lexists
    assert os.path.lexists(Path(__file__)) and not os.path.lexists(missing)
    assert os.path.isfile is posixpath.isfile is genericpath.isfile
    assert os.path.isjunction is posixpath.isjunction is genericpath.isjunction
    assert not os.path.isjunction(Path(missing))
    try:
        os.path.isjunction(42)
    except TypeError:
        pass
    else:
        raise AssertionError("isjunction accepted an invalid path")
    assert os.listdir is posix.listdir
    module_dir = os.path.dirname(__file__) or "."
    module_file = os.path.basename(__file__)
    assert os.path.realpath(__file__, strict=True) == os.path.realpath(__file__)
    assert os.path.realpath(os.fsencode(__file__), strict=os.path.ALLOW_MISSING) == os.fsencode(os.path.realpath(__file__))
    assert module_file in os.listdir(Path(module_dir))
    assert os.fsencode(module_file) in os.listdir(Path(os.fsencode(module_dir)))
    assert os.path.join(b"root", b"sub", b"") == b"root/sub/"
    assert os.path.join(b"root", b"before", b"/other", b"child") == b"/other/child"
    try:
        os.path.join(b"root", "text")
    except TypeError as error:
        assert str(error) == "Can't mix strings and bytes in path components"
    else:
        raise AssertionError("join accepted mixed bytes and text")
    assert os.path.join(Path("root"), "dirA") == "root/dirA"
    assert os.path.join("root", Path("/other"), Path("child")) == "/other/child"
    assert os.path.isabs(Path(b"/test"))
    assert abspath is os.path.abspath is posixpath.abspath
    assert abspath(path=Path(b"sub/../\xff")) == os.getcwdb().rstrip(b"/") + b"/\xff"
    assert abspath(Path(os.fsdecode(b"//host/\xff/./name/.."))) == os.fsdecode(b"//host/\xff")
    assert abspath(Raw(b"///host/../../dir")) == b"/dir"

    class Rooted(Text):
        def startswith(self, sep):
            assert sep == "/"
            return True

    assert abspath(Rooted("a/../relative")) == "relative"
    assert os.path.relpath(Path("/a/./b"), start="/a/c") == "../b"
    assert os.path.relpath(Path(b"/a/\xff"), start=b"/a") == b"\xff"
    try:
        os.listdir(__file__)
    except NotADirectoryError as error:
        assert error.filename == __file__
    else:
        raise AssertionError("listing a file must fail")
    assert os.path.isdir is posixpath.isdir is genericpath.isdir
    assert os.path.isdir(Path(b".")) and not os.path.isdir(Path("missing\0dir"))
    assert not os.path.isdir(-1)
    assert genericpath.exists.__module__ == "genericpath"
    assert os.path.exists(Path(b".")) and not os.path.exists(b"imre\0missing")
    assert not os.path.exists(-1)
    key = "IMRE_BOXED_GETENV_FIXTURE"
    missing = []
    assert os.getenv("not=a=variable", missing) is missing
    previous = os.getenv(key)
    try:
        os.environ[key] = ""
        os.putenv(key, "native only")
        assert os.getenv(key, missing) == ""
        assert posix.environ[key.encode()] == b""
        snapshot = os.environ.copy()
        assert type(snapshot) is dict
        assert snapshot[key] == ""
        snapshot[key] = "copy only"
        assert os.environ[key] == ""
        try:
            os.getenv(key.encode())
        except TypeError as error:
            assert str(error) == "str expected, not bytes"
        else:
            raise AssertionError("accepted a bytes environment key")
    finally:
        del os.environ[key]
        if previous is not None:
            os.environ[key] = previous
    assert os.path is posixpath and os.path.__name__ == "posixpath"
    assert sep is os.sep is path_sep is posixpath.sep and sep == "/"
    assert altsep is os.altsep is path_altsep is posixpath.altsep is None
    assert pathsep is os.pathsep is path_list_sep is os.path.pathsep is posixpath.pathsep
    assert linesep is os.linesep and linesep == "\n"
    assert not hasattr(posixpath, "linesep")
    assert os.extsep == os.path.extsep == posixpath.extsep == "."
    assert pathsep == ":"
    assert ":λ::some path:".split(pathsep) == ["", "λ", "", "some path", ""]
    assert os.path.splitext is posixpath.splitext and os.path.join is posixpath.join
    assert fspath is os.fspath is posix.fspath
    assert fspath.__self__ is posix and fspath.__module__ == "posix"
    for value in ("", ".profile", "a.b/.c", "a/.a.b", "/résumé.☃", "r/", b"a.\xff", Text("a.tar.gz"), Raw(b"a.tar.gz")):
        path = Path(value)
        assert fspath(path) is value and fspath(value) is value
        assert os.path.normcase(path) is value
        drive, tail = os.path.splitdrive(path)
        assert tail is value
        print(drive, os.path.basename(path), os.path.splitext(path))
    class ChangedSearch(str):
        def rfind(self, sep):
            if sep == "/":
                return None
            raise LookupError("dot search")

    try:
        posixpath.splitext(ChangedSearch("a.b"))
    except LookupError as error:
        print(str(error))
    else:
        raise AssertionError("the extension search must run")

    for value in (NoMethod(), Raises(), Path(42), None):
        try:
            fspath(value)
        except (TypeError, RuntimeError) as error:
            print(type(error).__name__, str(error))
    original = os.fspath
    calls = []
    def counted(value):
        calls.append(value)
        return original(value)
    try:
        os.fspath = counted
        for _ in range(3):
            os.path.normcase("werkzeug/serving.py")
        filename = os.path.basename("werkzeug/serving.py")
        assert os.path.splitext(filename) == ("serving", ".py")
    finally:
        os.fspath = original
    assert calls == ["werkzeug/serving.py"] * 4 + ["serving.py"]

    with open(Path(__file__), "rb", -1, None, None, None) as stream:
        assert stream.read(6) == b"import"
    with open(Path(__file__), encoding="utf-8", errors=None, newline=None) as stream:
        assert stream.readline() == "import genericpath\n"
    class DescriptorPath:
        def __index__(self):
            return fd

        def __fspath__(self):
            raise AssertionError("open selected the pathname of a valid descriptor")

    fd = os.open(__file__, os.O_RDONLY)
    try:
        with open(DescriptorPath(), "rb", closefd=False) as stream:
            assert stream.fileno() == fd
            assert stream.read(6) == b"import"
        assert os.lseek(fd, 0, os.SEEK_SET) == 0
    finally:
        os.close(fd)
    fd = os.open(__file__, os.O_RDONLY)
    with open(DescriptorPath(), "rb") as stream:
        assert stream.read(6) == b"import"
    try:
        os.fstat(fd)
    except OSError:
        pass
    else:
        os.close(fd)
        raise AssertionError("closing a buffered reader left its owned descriptor open")

    path = "imre-boxed-buffer-position"
    with open(path, "xb") as stream:
        stream.write(b"abcdefghij")
    try:
        for refresh, target, whence, position, next_byte, raw_position in (
            (False, 6, os.SEEK_SET, 6, b"g", 0),
            (True, 6, os.SEEK_SET, 6, b"g", 10),
            (True, 2, os.SEEK_CUR, 0, b"d", 0),
            (False, 13, os.SEEK_CUR, 4, b"e", 10),
            (False, 14, os.SEEK_SET, 14, b"", 14),
        ):
            with open(os.open(path, os.O_RDONLY), "rb") as stream:
                assert stream.read(1) == b"a"
                os.lseek(stream.fileno(), 0, os.SEEK_SET)
                if refresh:
                    assert stream.tell() == 0
                assert stream.seek(target, whence) == position
                assert stream.read(1) == next_byte
                assert os.lseek(stream.fileno(), 0, os.SEEK_CUR) == raw_position

        fd = os.open(path, os.O_WRONLY)
        stream = open(fd, "wb")
        stream.write(b"XX")
        stream.raw.close()
        replacement = os.open(path, os.O_RDWR)
        try:
            assert replacement == fd
            assert stream.close() is None
            with open(replacement, "rb", 0, closefd=False) as borrowed:
                assert borrowed.read() == b"abcdefghij"
        finally:
            os.close(replacement)

        with open(path, "wb") as stream:
            stream.write(b"first\r\nsecond\n\rlast")
        for newline in ("\n", "\r", "\r\n"):
            with open(Path(path), "r", -1, "utf-8", None, newline) as stream:
                assert stream.read() == "first\r\nsecond\n\rlast"

        calls = []
        class FallbackPath(Path):
            def __index__(self):
                calls.append("index")
                if len(calls) == 1:
                    raise TypeError("use the pathname")
                return alternate.fileno()

            def __fspath__(self):
                calls.append("fspath")
                return self.value

        value = FallbackPath(path)
        with open(__file__, "rb", buffering=0) as alternate:
            with open(value, "rt", encoding="utf-8", newline="\n") as stream:
                assert stream.name is value and stream.read() == "first\r\nsecond\n\rlast"
            assert calls == ["index", "fspath"]
            assert alternate.read(6) == b"import"
    finally:
        os.unlink(path)

    # Conversion runs before incompatible binary options are rejected.
    try:
        open(Raises(), "rb", -1, None, "replace", None)
    except RuntimeError as exc:
        assert str(exc) == "method call"
    else:
        raise AssertionError("open ignored the failing filesystem protocol")
    assert os.fsdecode(Path(b"caf\xc3\xa9")) == "café"
    assert not os.path.isfile(Path("missing\0file"))

    from os import PathLike
    from types import GenericAlias

    assert PathLike is os.PathLike and PathLike.__module__ == "os"
    assert PathLike.__abstractmethods__ == frozenset(("__fspath__",))
    alias = PathLike[str]
    assert type(alias) is GenericAlias and alias.__origin__ is PathLike
    assert alias.__args__ == (str,)
    assert (str | bytes | alias).__args__ == (str, bytes, alias)

    class Slotted(PathLike):
        __slots__ = ()

        def __fspath__(self):
            return b"native"

    class RefusesSubclasses(Slotted):
        @classmethod
        def __subclasshook__(cls, child):
            return False

    assert isinstance(RefusesSubclasses(), RefusesSubclasses)
    assert not issubclass(RefusesSubclasses, RefusesSubclasses)

    class Disabled(Path):
        __fspath__ = None

    assert issubclass(Path, PathLike) and isinstance(Path("x"), PathLike)
    assert PathLike.__subclasshook__(NoMethod) is True
    assert PathLike.__subclasshook__(Disabled) is NotImplemented
    assert not issubclass(Disabled, PathLike) and not issubclass(Path, Slotted)
    assert not hasattr(Slotted(), "__dict__") and os.fspath(Slotted()) == b"native"

    class Virtual:
        pass

    assert PathLike.register(Virtual) is Virtual
    assert issubclass(Virtual, PathLike) and isinstance(Virtual(), PathLike)
