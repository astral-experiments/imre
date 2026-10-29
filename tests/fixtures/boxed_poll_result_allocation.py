import os
import sys

select = __import__(sys.argv[2])

count = int(sys.argv[1])
pipes = [os.pipe() for _ in range(count)]
poller = select.poll()
for read_fd, write_fd in pipes:
    poller.register(write_fd, select.POLLOUT)
expected = [(write_fd, select.POLLOUT) for read_fd, write_fd in pipes]
try:
    ready = poller.poll(0)
    print('ok')
    print(ready == expected)
except MemoryError:
    print('MemoryError')
print(poller.poll(0) == expected)
for read_fd, write_fd in pipes:
    os.close(read_fd)
    os.close(write_fd)
