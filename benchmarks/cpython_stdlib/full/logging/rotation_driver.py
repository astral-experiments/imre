"""Keep the native handler, descriptor and records alive throughout each method."""

import logging.handlers
from logging.handlers import RotatingFileHandler


def run() -> None:
    handlers: list[RotatingFileHandler] = []
    records: list[logging.LogRecord] = []
    while True:
        command = input()
        if command == "stop":
            return
        if command == "debug":
            print(logging.DEBUG)
        elif command == "open":
            filename = input()
            mode = input()
            limit = int(input())
            backups = int(input())
            codec = input()
            delayed = input() == "1"
            handler = RotatingFileHandler(filename, mode, limit, backups, codec, delayed)
            handlers.append(handler)
            print(len(handlers) - 1)
        elif command == "create":
            name = input()
            level = int(input())
            path = input()
            line = int(input())
            msg = input()
            records.append(logging.LogRecord(name, level, path, line, msg, None, None, None))
            print(len(records) - 1)
        elif command == "from_message":
            records.append(logging.makeLogRecord({"msg": input()}))
            print(len(records) - 1)
        elif command == "message":
            print(repr(records[int(input())].msg))
        elif command == "should":
            handler = handlers[int(input())]
            index = int(input())
            if index == -1:
                print(handler.shouldRollover(None))
            else:
                print(handler.shouldRollover(records[index]))
        elif command == "emit":
            handler = handlers[int(input())]
            handler.emit(records[int(input())])
            print("None")
        elif command == "close":
            handlers[int(input())].close()
            print("None")
        else:
            raise ValueError(command)


run()
