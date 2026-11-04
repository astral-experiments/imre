
from contextvars import ContextVar as _ContextVar
from collections.abc import MutableMapping as _MutableMapping
from sys import maxsize as _maxsize


class Clamped(DecimalException):
    pass


class Rounded(DecimalException):
    pass


class Inexact(DecimalException):
    pass


class Subnormal(DecimalException):
    pass


class Overflow(Inexact, Rounded):
    pass


class Underflow(Inexact, Rounded, Subnormal):
    pass


class DivisionByZero(DecimalException, ZeroDivisionError):
    pass


class FloatOperation(DecimalException, TypeError):
    pass


# Iteration and trap priority follow libmpdec's signal order.
_signals = (InvalidOperation, FloatOperation, DivisionByZero, Overflow,
            Underflow, Subnormal, Inexact, Rounded, Clamped)
ROUND_CEILING = "ROUND_CEILING"
ROUND_FLOOR = "ROUND_FLOOR"
ROUND_UP = "ROUND_UP"
ROUND_DOWN = "ROUND_DOWN"
ROUND_HALF_UP = "ROUND_HALF_UP"
ROUND_HALF_DOWN = "ROUND_HALF_DOWN"
ROUND_HALF_EVEN = "ROUND_HALF_EVEN"
ROUND_05UP = "ROUND_05UP"
_rounding_modes = (ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN,
                   ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN, ROUND_05UP)
MAX_PREC = 999999999999999999 if _maxsize > 2**32 else 425000000
MAX_EMAX = MAX_PREC
MIN_EMIN = -MAX_EMAX
MIN_ETINY = MIN_EMIN - MAX_PREC + 1
_limits = {"prec": (1, MAX_PREC), "Emin": (MIN_EMIN, 0), "Emax": (0, MAX_EMAX),
           "capitals": (0, 1), "clamp": (0, 1)}


class _SignalDict(_MutableMapping):
    """A context owns its nine signal keys for the lifetime of the context."""

    def __init__(self, values):
        self._values = {signal: False for signal in _signals}
        if isinstance(values, list):
            for signal in values:
                self[signal] = True
        else:
            self._replace(values)

    def _replace(self, values):
        if not isinstance(values, (dict, _SignalDict)):
            raise TypeError("argument must be a signal dict")
        if len(values) != len(_signals) or any(s not in values for s in _signals):
            raise KeyError("invalid signal dict")
        self._values.update({signal: bool(values[signal]) for signal in _signals})

    def __getitem__(self, key):
        return self._values[key]

    def __setitem__(self, key, value):
        if key not in self._values:
            raise KeyError("invalid signal")
        self._values[key] = bool(value)

    def __delitem__(self, key):
        raise ValueError("signal keys cannot be deleted")

    def __iter__(self):
        return iter(self._values)

    def __len__(self):
        return len(self._values)

    def copy(self):
        return self._values.copy()


class Context:
    __slots__ = ("prec", "rounding", "Emin", "Emax", "capitals", "clamp", "flags", "traps")

    def __init__(self, prec=None, rounding=None, Emin=None, Emax=None, capitals=None,
                 clamp=None, flags=None, traps=None):
        self.prec = DefaultContext.prec if prec is None else prec
        self.rounding = DefaultContext.rounding if rounding is None else rounding
        self.Emin = DefaultContext.Emin if Emin is None else Emin
        self.Emax = DefaultContext.Emax if Emax is None else Emax
        self.capitals = DefaultContext.capitals if capitals is None else capitals
        self.clamp = DefaultContext.clamp if clamp is None else clamp
        object.__setattr__(self, "flags", _SignalDict(DefaultContext.flags if flags is None else flags))
        object.__setattr__(self, "traps", _SignalDict(DefaultContext.traps if traps is None else traps))

    def __setattr__(self, name, value):
        if name in _limits:
            if not issubclass(type(value), int):
                raise TypeError("an integer is required")
            value = int.__index__(value)
            if value < -_maxsize-1 or value > _maxsize:
                raise OverflowError("Python int too large to convert to C ssize_t")
            low, high = _limits[name]
            if not low <= value <= high:
                raise ValueError("value out of range in " + name)
        elif name == "rounding":
            # Compare the stored string, including for subclasses that override equality.
            for mode in _rounding_modes:
                if str.__eq__(mode, value) is True:
                    value = mode
                    break
            else:
                raise TypeError(
                    "valid values for rounding are:\n"
                    "  [ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN,\n"
                    "   ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN,\n"
                    "   ROUND_05UP]"
                )
        elif name == "flags" or name == "traps":
            getattr(self, name)._replace(value)
            return
        object.__setattr__(self, name, value)

    def copy(self):
        return Context(self.prec, self.rounding, self.Emin, self.Emax, self.capitals,
                       self.clamp, self.flags, self.traps)

    def clear_flags(self):
        for s in _signals:
            self.flags[s] = False

    def clear_traps(self):
        for s in _signals:
            self.traps[s] = False

    def Etiny(self):
        return self.Emin - self.prec + 1

    def Etop(self):
        return self.Emax - self.prec + 1

    def _signal(self, *signals):
        for signal in signals:
            self.flags[signal] = True
        trapped = [s for s in _signals if s in signals and self.traps[s]]
        if trapped:
            raise trapped[0](trapped)


DefaultContext = Context(28, ROUND_HALF_EVEN, -999999, 999999, 1, 0, [],
                         [InvalidOperation, DivisionByZero, Overflow])
BasicContext = Context(9, ROUND_HALF_UP, traps=[InvalidOperation, DivisionByZero,
                                               Overflow, Underflow, Clamped])
ExtendedContext = Context(9, traps=[])
_current_context = _ContextVar("decimal_context", default=None)


def getcontext():
    context = _current_context.get()
    if context is None:
        context = DefaultContext.copy()
        context.clear_flags()
        _current_context.set(context)
    return context


def setcontext(context, /):
    if not isinstance(context, Context):
        raise TypeError("argument must be a context")
    if context is DefaultContext or context is BasicContext or context is ExtendedContext:
        context = context.copy()
        context.clear_flags()
    _current_context.set(context)


class _ContextManager:
    def __init__(self, context):
        self.previous = getcontext()
        self.context = context

    def __enter__(self):
        _current_context.set(self.context)
        return self.context

    def __exit__(self, exc_type, exc_value, traceback):
        _current_context.set(self.previous)


def localcontext(ctx=None, **kwargs):
    if ctx is None:
        ctx = getcontext()
    if not isinstance(ctx, Context):
        raise TypeError("optional argument must be a context")
    result = ctx.copy()
    for key, val in kwargs.items():
        if key not in Context.__slots__:
            raise TypeError("this function got an unexpected keyword argument " + repr(key))
        if val is not None:
            if key == "flags" or key == "traps":
                val = _SignalDict(val)
            setattr(result, key, val)
    return _ContextManager(result)


def _display(text):
    if "E" in text and not getcontext().capitals:
        return text.replace("E", "e")
    return text
