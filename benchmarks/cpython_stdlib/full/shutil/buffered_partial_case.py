"""Recover a partially flushed buffered writer and seek from its logical position."""
import os
import sys

reader = int(sys.argv[1])
writer = int(sys.argv[2])
payload = b'\x00abcdefgh\xff' * 3000
with open('recovered.bin', 'wb') as stream:
    fd = stream.fileno()
    saved = os.dup(fd)
    os.lseek(saved, 5, os.SEEK_SET)
    os.dup2(writer, fd)
    os.close(writer)
    try:
        stream.write(payload)
        stream.flush()
    except OSError as error:
        print(repr(error.errno))
        print('exception-metadata ' + repr(error))
    try:
        stream.seek(0)
    except OSError as error:
        print(repr(error.errno))
        print('exception-metadata ' + repr(error))
    os.dup2(saved, fd)
    os.close(saved)
    with open('/proc/self/fd/' + str(reader), 'rb') as pipe:
        os.set_blocking(pipe.fileno(), False)
        prefix = pipe.read(4096)
    os.close(reader)
    print(len(prefix))
    print(stream.tell() == 5 + len(payload) - len(prefix))
    with open('recovered.bin', 'rb') as unflushed:
        print(unflushed.read() == b'')
    print(stream.seek(2, os.SEEK_CUR) == 7 + len(payload) - len(prefix))
    stream.write(b'!')
with open('recovered.bin', 'rb') as recovered:
    print(prefix == payload[:len(prefix)] and recovered.read() == b'\x00' * 5 + payload[len(prefix):] + b'\x00\x00!')
