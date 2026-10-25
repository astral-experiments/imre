"""Compare exact names, hierarchy boundaries and independently mutable filter fields."""

import logging
from logging import Filter as NamedFilter


class Reject(NamedFilter):
    def filter(self, record: logging.LogRecord) -> bool:
        return False


scope = NamedFilter(name="π.service")
print(scope.name)
print(scope.nlen)
for key in ["π", "π.service", "π.service.worker", "π.services", "π.service.", "π..service"]:
    record = logging.makeLogRecord({"name": key})
    print(key)
    print(scope.filter(record))

unnamed = logging.makeLogRecord({})
print(logging.Filter().filter(unnamed))
try:
    scope.filter(unnamed)
except AttributeError as error:
    print(str(error))
scope.name = "one"
scope.nlen = 3
print(scope.filter(logging.makeLogRecord({"name": "one.two"})))
print(Reject().filter(unnamed))
