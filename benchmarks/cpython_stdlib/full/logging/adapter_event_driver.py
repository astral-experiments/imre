"""Keep one native root, adapter and recording handler alive for one original host method."""
import logging


class Recording(logging.Handler):
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

    def emit(self, record: logging.LogRecord | None) -> None:
        if record is not None:
            self.records.append(record)


def context() -> dict[str, str] | None:
    n = int(input())
    if n < 0:
        return None
    result: dict[str, str] = {}
    for _ in range(n):
        key = input()
        result[key] = input()
    return result


def run() -> None:
    root = logging.root
    root.setLevel(int(input()))
    recording = Recording()
    root.addHandler(recording)
    adapter = logging.LoggerAdapter(root)
    while True:
        command = input()
        if command == 'stop':
            return
        if command == 'adapter':
            merging = input() == 'True'
            adapter = logging.LoggerAdapter(root, context(), merge_extra=merging)
            print('ready')
        elif command == 'critical':
            message = input()
            if input() == 'present':
                adapter.critical(message, extra=context())
            else:
                adapter.critical(message)
            print('logged')
        elif command == 'length':
            print(str(len(recording.records)))
        elif command == 'index':
            i = int(input())
            if i < 0:
                i += len(recording.records)
            if 0 <= i and i < len(recording.records):
                print(str(i))
            else:
                print('missing')
        elif command == 'attribute':
            record = recording.records[int(input())]
            key = input()
            if hasattr(record, key):
                print('value ' + repr(record.__dict__[key]))
            else:
                print('missing')
        else:
            raise ValueError('unknown record command')


run()
