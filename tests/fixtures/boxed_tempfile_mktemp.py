import errno
import os
import tempfile


def current():
    directory = tempfile.mkdtemp(dir=".")
    occupied = os.path.join(directory, "prefix-used.suf")
    os.close(os.open(occupied, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600))
    original = tempfile._get_candidate_names
    try:
        tempfile._get_candidate_names = lambda: iter(("used", "free"))
        name = tempfile.mktemp(".suf", "prefix-", directory)
        assert name == os.path.join(directory, "prefix-free.suf")
        assert not os.path.lexists(name)
        names = iter(["used"] * tempfile.TMP_MAX + ["unused"])
        tempfile._get_candidate_names = lambda: names
        try:
            tempfile.mktemp(".suf", "prefix-", directory)
        except FileExistsError as error:
            assert error.errno == errno.EEXIST
            assert next(names) == "unused"
        else:
            raise AssertionError("colliding names did not exhaust TMP_MAX")
        assert os.listdir(directory) == ["prefix-used.suf"]
    finally:
        tempfile._get_candidate_names = original
        os.unlink(occupied)
        os.rmdir(directory)
    print("mktemp name selection checked")
