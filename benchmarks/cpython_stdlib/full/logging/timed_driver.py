"""Stateful native operations for the upstream timed file handler methods."""
import datetime
import logging.handlers
import sys


def main() -> None:
    handlers: list[logging.handlers.TimedRotatingFileHandler] = []
    while True:
        command = input()
        if command == "stop":
            return
        try:
            if command == "new":
                name = input()
                when = input()
                interval = int(input())
                backups = int(input())
                delay = input() == "1"
                utc = input() == "1"
                clock: datetime.time | None = None
                if input() == "1":
                    hour = int(input())
                    minute = int(input())
                    second = int(input())
                    microsecond = int(input())
                    fold = int(input())
                    clock = datetime.time(hour, minute, second, microsecond, fold=fold)
                handlers.append(logging.handlers.TimedRotatingFileHandler(
                    name, when, interval, backups, encoding="utf-8", delay=delay, utc=utc, atTime=clock))
                print(len(handlers) - 1)
            else:
                handler = handlers[int(input())]
                if command == "int":
                    print(handler.computeRollover(int(input())))
                elif command == "float":
                    print(handler.computeRollover(float(input())))
                elif command == "deletions":
                    print(repr(handler.getFilesToDelete()))
                elif command == "close":
                    handler.close()
                    print("None")
                else:
                    raise RuntimeError("unexpected test command")
        except ValueError as exc:
            print("ValueError:" + repr(str(exc)))
        except OverflowError as exc:
            print("OverflowError:" + repr(str(exc)))
        sys.stdout.flush()


main()
