import os
import tempfile


def current():
    directory = os.path.abspath("tempdir-probe")
    os.mkdir(directory)
    write = os.write

    def fail_write(fd, data):
        raise OSError("probe write failed")

    try:
        assert tempfile._get_default_tempdir([directory]) == directory
        assert os.listdir(directory) == []
        os.write = fail_write
        try:
            tempfile._get_default_tempdir([directory])
        except FileNotFoundError:
            pass
        else:
            raise AssertionError("a failed write selected the directory")
        assert os.listdir(directory) == []
    finally:
        os.write = write
        os.rmdir(directory)


if __name__ == "__main__":
    current()
