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
        print(error.errno)
    try:
        stream.seek(0)
    except OSError as error:
        print(error.errno)
    os.dup2(saved, fd)
    os.close(saved)
    prefix = os.read(reader, len(payload))
    os.close(reader)
    print(len(prefix))
    print(stream.tell() == 5 + len(payload) - len(prefix))
    print(os.path.getsize('recovered.bin') == 0)
    print(stream.seek(2, os.SEEK_CUR) == 7 + len(payload) - len(prefix))
    stream.write(b'!')
with open('recovered.bin', 'rb') as recovered:
    print(recovered.read() == b'\x00' * 5 + payload[len(prefix):] + b'\x00\x00!')
