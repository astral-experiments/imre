import socket
import sys


def main() -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(8)
        host, port = listener.getsockname()
        print(port)
        sys.stdout.flush()
        conn, addr = listener.accept()
        with conn:
            print(host == addr[0] and port > 0 and addr[1] > 0)
            first = conn.recv(1)
            suffix = conn.recv(4096)
            while len(suffix) < 4:
                more = conn.recv(4096)
                if not more:
                    break
                suffix = suffix + more
            print(first)
            print(suffix)
            print(conn.recv(1))
            conn.sendall(suffix + first)
        print(conn.fileno())
    print(listener.fileno())


main()
