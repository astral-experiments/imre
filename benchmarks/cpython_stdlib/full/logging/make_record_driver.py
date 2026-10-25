"""Keep factory-created LogRecords alive while the original CPython method inspects them."""
import logging


def serve() -> None:
    logger = logging.getLogger("blah")
    records: list[logging.LogRecord] = []
    while True:
        command = input()
        if command == "stop":
            return
        if command == "keys":
            index = int(input())
            print(list(records[index].__dict__))
        elif command == "factory":
            name = input()
            level = int(input())
            records.append(logging._logRecordFactory(name, level, None, None, None, None, None, None, None))
            print(len(records) - 1)
        elif command == "make":
            name = input()
            level = int(input())
            key = input()
            value = input()
            try:
                records.append(logger.makeRecord(name, level, None, None, None, None, None,
                                                 extra={key: value}, sinfo=None))
                print("record " + str(len(records) - 1))
            except KeyError as error:
                print("key-error " + str(error))
        else:
            raise ValueError(command)


serve()
