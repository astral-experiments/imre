from collections import deque
import sys

def main() -> None:
    queues: list[deque[float]] = []
    while True:
        cmd = input()
        if cmd == "quit":
            return
        if cmd == "new":
            values: list[float] = []
            for i in range(int(input())):
                values.append(int(input()) / 9007199254740992)
            queues.append(deque(values))
            print(len(queues) - 1)
        else:
            q = queues[int(input())]
            if cmd == "reverse":
                print(q.reverse())
            elif cmd == "snapshot":
                words: list[int] = []
                for value in q:
                    words.append(int(value * 9007199254740992))
                print(words)
            elif cmd == "length":
                print(len(q))
            elif cmd == "bad-arity":
                try:
                    q.reverse(int(input()))
                except TypeError as exc:
                    print("TypeError:" + str(exc))
                else:
                    print("unexpected return")
            else:
                raise ValueError("unknown driver operation")
        sys.stdout.flush()

main()
