import os
import sys
import tempfile
from tempfile import TemporaryDirectory


def current():
    assert TemporaryDirectory is tempfile.TemporaryDirectory
    with TemporaryDirectory(dir=".", delete=False) as kept:
        owner = TemporaryDirectory(dir=kept)
        fd = os.open(os.path.join(owner.name, "file"), os.O_CREAT | os.O_RDWR, 0o600)
        try:
            with owner as directory:
                assert directory == owner.name
                os.mkdir(os.path.join(directory, "child"))
                os.symlink(kept, os.path.join(directory, "link"))
            assert not os.path.exists(directory)
            assert os.write(fd, b"open") == 4
            owner.cleanup()
        finally:
            os.close(fd)
        denied = FileExistsError("directory creation denied")

        def reject(event, args):
            if event == "tempfile.mkdtemp" and os.path.dirname(args[0]) == kept:
                assert not os.path.exists(args[0])
                raise denied

        sys.addaudithook(reject)
        try:
            with TemporaryDirectory(dir=kept):
                raise AssertionError("expected mkdtemp audit hook to reject the directory")
        except FileExistsError as error:
            assert error is denied
        assert os.listdir(kept) == []
    assert os.path.isdir(kept)
    os.rmdir(kept)
