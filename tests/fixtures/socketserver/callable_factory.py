from socketserver import BaseRequestHandler, TCPServer


class Handler(BaseRequestHandler):
    def handle(self):
        calls.append(self.client_address[1])


def make_handler(request, address, server):
    calls.append(1)
    return Handler(request, address, server)


calls = bytearray()
with TCPServer(("127.0.0.1", 0), make_handler, False) as server:
    server.finish_request(server.socket, ("127.0.0.1", 23))
print(list(calls))
