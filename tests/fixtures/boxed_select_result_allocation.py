import os
import select
import sys

shape = int(sys.argv[1])
count = int(sys.argv[2]) if len(sys.argv) > 2 else 1
readers = []
writers = []
exceptions = []
expected = ([], [], [])
if shape:
    read_fd, eof_write_fd = os.pipe()
    other_read_fd, write_fd = os.pipe()
    os.close(eof_write_fd)
    if shape == 1:
        readers = [read_fd] * count
        expected = ([read_fd] * count, [], [])
    elif shape == 2:
        writers = [write_fd] * count
        expected = ([], [write_fd] * count, [])
    else:
        readers = [read_fd] * count
        writers = [write_fd] * count
        exceptions = [read_fd]
        expected = ([read_fd] * count, [write_fd] * count, [])
marker = 'arm select result refusal'
try:
    ready = select.select(readers, writers, exceptions, 0)
    print('ok')
    print(ready == expected)
except MemoryError:
    print('MemoryError')
print(select.select(readers, writers, exceptions, 0) == expected)
if shape:
    os.close(read_fd)
    os.close(write_fd)
    os.close(other_read_fd)
