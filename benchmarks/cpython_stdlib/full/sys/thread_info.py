"""Read the live thread record in the target process, including optional fields."""
import sys
from sys import thread_info


def same():
    return thread_info is sys.thread_info


def main():
    info = sys.thread_info
    operation = sys.argv[1]
    if operation == "thread_info":
        print(repr(info[:]))
    elif operation == "length":
        print(len(info))
    elif operation == "name":
        print(repr(info.name))
    elif operation == "lock":
        print(repr(info.lock))
    elif operation == "platform":
        print(repr(sys.platform))
    elif operation == "describe":
        print(same())
        print(info is sys.thread_info)
        print(repr((info.name, info.lock, info.version)))
        print(repr(info.__match_args__))
        print(repr(info[:]))
        print(repr(info))
    else:
        raise ValueError("unknown thread-info operation")


main()
