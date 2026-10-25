import os
from pathlib import Path


def seek(fd: int, position: int, whence: int) -> None:
    try:
        print(os.lseek(fd, position, whence))
    except OverflowError as error:
        print("overflow " + str(error))
    except OSError as error:
        print("os error " + str(error))


path = Path("seek-log.txt")
with path.open("w", encoding="utf-8") as output:
    output.write("πabc")
    output.flush()
    fd = output.fileno()
    alias = os.dup(fd)
    try:
        seek(fd, 0, os.SEEK_CUR)
        seek(alias, -3, os.SEEK_END)
        seek(fd, 0, os.SEEK_CUR)
        print(os.lseek(fd, False, os.SEEK_SET))
        seek(alias, 0, os.SEEK_CUR)
        seek(fd, -1, os.SEEK_SET)
        seek(alias, 0, os.SEEK_CUR)
        seek(fd, 0, os.SEEK_DATA)
        seek(fd, 0, os.SEEK_HOLE)
        seek(fd, 5, os.SEEK_DATA)
    finally:
        os.close(alias)

# Positions fitting off_t but exceeding Imre's immediate integer range still
# reach the kernel. Out-of-range C inputs raise before the descriptor is used.
seek(-1, 1 << 62, os.SEEK_SET)
seek(-1, (1 << 63) - 1, os.SEEK_SET)
seek(-1, 1 << 63, os.SEEK_SET)
seek(-1, -(1 << 63) - 1, os.SEEK_SET)
seek(1 << 70, 1 << 70, 1 << 70)
seek(-1, 0, 1 << 40)
reader, writer = os.pipe()
try:
    seek(reader, 0, os.SEEK_CUR)
    seek(writer, 0, os.SEEK_CUR)
finally:
    os.close(reader)
    os.close(writer)
