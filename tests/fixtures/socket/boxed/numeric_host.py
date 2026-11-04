import socket
from _socket import AI_NUMERICHOST as raw_flag, AI_V4MAPPED as raw_mapped
from socket import AF_INET, AI_NUMERICHOST, AI_V4MAPPED, SOCK_STREAM, getaddrinfo

assert type(raw_flag) is int
assert AI_NUMERICHOST is socket.AddressInfo.AI_NUMERICHOST
assert AI_NUMERICHOST.value == raw_flag

rows = getaddrinfo("127.0.0.1", 80, AF_INET, SOCK_STREAM, flags=AI_NUMERICHOST)
assert rows[0][4] == ("127.0.0.1", 80)
try:
    getaddrinfo("localhost", 80, AF_INET, SOCK_STREAM, flags=AI_NUMERICHOST)
except socket.gaierror as exc:
    assert exc.errno == socket.EAI_NONAME
else:
    raise AssertionError("AI_NUMERICHOST accepted a host name")

assert getaddrinfo("localhost", 80, AF_INET, SOCK_STREAM)

assert type(raw_mapped) is int
assert AI_V4MAPPED is socket.AddressInfo.AI_V4MAPPED
assert AI_V4MAPPED.value == raw_mapped
(mapped,) = getaddrinfo("127.0.0.1", 80, socket.AF_INET6, socket.SOCK_DGRAM,
                        flags=AI_NUMERICHOST | AI_V4MAPPED)
assert mapped[4] == ("::ffff:127.0.0.1", 80, 0, 0)
