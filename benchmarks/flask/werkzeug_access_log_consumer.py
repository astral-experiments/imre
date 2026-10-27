import wheel_werkzeug_internal as werkzeug
import logging
import sys

werkzeug._log("info", "GET %s %d", "/health", 200)
first = logging.getLogger("werkzeug")
werkzeug._log("info", "GET %s %d", "/missing", 404)
print(werkzeug._logger is first)
print(first.level == logging.INFO)
print(len(first.handlers), werkzeug._has_level_handler(first))
print(isinstance(first.handlers[0], logging.StreamHandler))
print(first.handlers[0].stream is sys.stderr)
