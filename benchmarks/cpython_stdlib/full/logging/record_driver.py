"""Persistent native operations used by LogRecordTest.test_optional."""
import logging


def run() -> None:
    records: list[logging.LogRecord] = []
    while True:
        op = input()
        if op == "stop":
            return
        if op == "new":
            records.append(logging.makeLogRecord({}))
            print(len(records) - 1)
        elif op == "get":
            field = input()
            if field == "logThreads":
                print(logging.logThreads)
            elif field == "logProcesses":
                print(logging.logProcesses)
            elif field == "logMultiprocessing":
                print(logging.logMultiprocessing)
            elif field == "logAsyncioTasks":
                print(logging.logAsyncioTasks)
            else:
                raise ValueError(field)
        elif op == "set":
            field = input()
            enabled = input() == "True"
            if field == "logThreads":
                logging.logThreads = enabled
            elif field == "logProcesses":
                logging.logProcesses = enabled
            elif field == "logMultiprocessing":
                logging.logMultiprocessing = enabled
            elif field == "logAsyncioTasks":
                logging.logAsyncioTasks = enabled
            else:
                raise ValueError(field)
            print("None")
        elif op == "field":
            r = records[int(input())]
            field = input()
            if field == "thread":
                print(repr(r.thread))
            elif field == "process":
                print(repr(r.process))
            elif field == "threadName":
                print(repr(r.threadName))
            elif field == "processName":
                print(repr(r.processName))
            elif field == "taskName":
                print(repr(r.taskName))
            else:
                raise ValueError(field)
        else:
            raise ValueError(op)


run()
