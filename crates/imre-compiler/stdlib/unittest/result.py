"""Test result reporting for the boxed unittest package."""

import sys
import traceback

from unittest import TestResult

__unittest = True
_stdlib_traceback = traceback


# Adapted from CPython 3.14.5 Lib/unittest/result.py. See ../docstrings/LICENSE.
def _bufferedOutput(output, label):
    if not output.endswith("\n"):
        output += "\n"
    return "\n%s:\n%s" % (label, output)


def _format_frame_locals(frame):
    from traceback import _frame_locals, _safe_string

    local_reprs = {name: _safe_string(value, 'local', repr)
                   for name, value in _frame_locals(frame).items()}
    return ''.join(f'    {name} = {value}\n' for name, value in sorted(local_reprs.items()))


def _exc_info_to_string(self, err, test):
    """Converts a sys.exc_info()-style tuple of values into a string."""
    exctype, value, tb = err
    tb = self._clean_tracebacks(exctype, value, tb, test)
    if traceback is _stdlib_traceback and not hasattr(traceback, "TracebackException"):
        # Boxed traceback provides native frame positions and embedded source.
        from _imre_unittest import format_exception
        msgLines = format_exception(
            exctype, value, tb, _format_frame_locals if self.tb_locals else None)
    else:
        report = traceback.TracebackException(
            exctype, value, tb, capture_locals=self.tb_locals, compact=True)
        from _colorize import can_colorize
        colorize = hasattr(self, "stream") and can_colorize(file=self.stream)
        msgLines = list(report.format(colorize=colorize))

    if self.buffer:
        output = sys.stdout.getvalue()
        error = sys.stderr.getvalue()
        if output:
            msgLines.append(_bufferedOutput(output, "Stdout"))
        if error:
            msgLines.append(_bufferedOutput(error, "Stderr"))
    return ''.join(msgLines)
