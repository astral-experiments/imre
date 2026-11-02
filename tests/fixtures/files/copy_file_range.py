import errno
import os


class Index:
    def __init__(self, label: str, value: int) -> None:
        self.label = label
        self.value = value

    def __index__(self) -> int:
        print("index " + self.label)
        return self.value


def argument(label: str, value: int) -> Index:
    print("argument " + label)
    return Index(label, value)


def copy(src: int, dst: int, count: int, offset: Index | None) -> int:
    return os.copy_file_range(src, dst, count, offset)


src = os.open("range-source", os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
try:
    dst = os.open("range-destination", os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        os.write(src, b"abcdef")
        os.lseek(src, 4, os.SEEK_SET)
        assert os.copy_file_range(offset_dst=argument("offset_dst", 2), count=argument("count", 2),
                                  src=argument("src", src), offset_src=argument("offset_src", 1),
                                  dst=argument("dst", dst)) == 2
        assert os.lseek(src, 0, os.SEEK_CUR) == 4
        assert os.lseek(dst, 0, os.SEEK_CUR) == 0
        assert copy(src, dst, 4, None) == 2
        assert copy(src, dst, 1, None) == 0
        assert copy(src, dst, 1, Index("explicit-zero", 0)) == 1
        assert os.lseek(src, 0, os.SEEK_CUR) == 6
        assert os.lseek(dst, 0, os.SEEK_CUR) == 3
        os.lseek(dst, 0, os.SEEK_SET)
        assert os.read(dst, 10) == b"efac"

        try:
            os.copy_file_range(-1, dst, 0)
        except OSError as exc:
            assert exc.errno == errno.EBADF
        else:
            raise AssertionError("an invalid descriptor accepted a zero count")
        try:
            os.copy_file_range(src, dst, -1, Index("unreached", 0))
        except ValueError as exc:
            print(str(exc))
        else:
            raise AssertionError("a negative count reached the syscall")
    finally:
        os.close(dst)
        os.unlink("range-destination")
finally:
    os.close(src)
    os.unlink("range-source")
