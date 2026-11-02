import socket
from socket import AF_INET6, has_ipv6

assert socket.has_ipv6 is has_ipv6 is True
assert socket.AF_INET6 == AF_INET6
print(int(AF_INET6))
