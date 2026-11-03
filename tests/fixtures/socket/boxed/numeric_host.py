import socket
from _socket import AI_NUMERICHOST as raw_flag
from socket import AF_INET, AI_NUMERICHOST, SOCK_STREAM, getaddrinfo

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
