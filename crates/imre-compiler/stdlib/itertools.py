"""Iterator tools for runtime-loaded Python."""


import _imre_itertools as _itertools
from operator import index as _index
from sys import maxsize as _maxsize


def _make_chain(_advance, _set, _allocate):
    class chain:
        """Yield items from each iterable in order."""

        __slots__ = ("_sources", "_active")

        def __init__(self, *iterables):
            _set(self, False, iter(iterables))
            _set(self, True, None)

        @classmethod
        def from_iterable(cls, iterable, /):
            return _allocate(__class__, cls, iter(iterable))

        def __iter__(self):
            return self

        def __next__(self):
            return _advance(__class__, self)


    chain.__qualname__ = "chain"
    return chain


chain = _make_chain(_itertools._chain_next, _itertools._chain_set, _itertools._chain_allocate)


def _make_cycle(_allocate, _advance):
    class cycle:
        """Repeat an iterable, saving each element as the input produces it."""

        __slots__ = ()

        def __new__(cls, *args, **kwargs):
            if kwargs and getattr(cls, "__init__", None) is getattr(__class__, "__init__", None):
                raise TypeError("cycle() takes no keyword arguments")
            if len(args) != 1:
                raise TypeError(f"cycle expected 1 argument, got {len(args)}")
            return _allocate(__class__, cls, args[0])

        def __iter__(self):
            return self

        def __next__(self):
            return _advance(self)

    cycle.__qualname__ = "cycle"
    return cycle


cycle = _make_cycle(_itertools._cycle_allocate, _itertools._cycle_next)


def _make_filterfalse(_allocate, _state):
    class filterfalse:
        """Yield items for which the predicate is false, testing items directly for None."""

        __slots__ = ()

        def __new__(cls, /, *args, **kwargs):
            if kwargs and getattr(cls, "__init__", None) is getattr(__class__, "__init__", None):
                raise TypeError("filterfalse() takes no keyword arguments")
            if len(args) != 2:
                raise TypeError(f"filterfalse expected 2 arguments, got {len(args)}")
            predicate, iterable = args
            return _allocate(__class__, cls, bool if predicate is None else predicate, iterable)

        def __iter__(self):
            return self

        def __next__(self):
            source, predicate = _state(__class__, self)
            while True:
                item = next(source)
                if not predicate(item):
                    return item

    filterfalse.__qualname__ = "filterfalse"
    return filterfalse


filterfalse = _make_filterfalse(_itertools._predicate_allocate, _itertools._predicate_state)


def _make_accumulate(_allocate, _advance):
    class accumulate:
        """Return series of accumulated sums (or other binary function results)."""

        __slots__ = ()

        def __new__(cls, /, iterable, func=None, *, initial=None):
            if not issubclass(cls, __class__):
                raise TypeError("expected an accumulate subclass")
            return _allocate(__class__, cls, iter(iterable), func, initial)

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                object.__init__(self, *args, **kwargs)

        def __iter__(self):
            return self

        def __next__(self):
            return _advance(__class__, self)

    accumulate.__qualname__ = "accumulate"
    return accumulate


accumulate = _make_accumulate(_itertools._accumulate_allocate, _itertools._accumulate_next)


def _make_starmap(_allocate, _advance):
    class starmap:
        """Apply a function to the arguments from each input row."""

        __slots__ = ()

        def __new__(cls, /, *args, **kwargs):
            if cls is not __class__:
                if not isinstance(cls, type):
                    raise TypeError(f"itertools.starmap.__new__(X): X is not a type object ({type(cls).__name__})")
                if not issubclass(cls, __class__):
                    raise TypeError(f"itertools.starmap.__new__({cls.__name__}): {cls.__name__} is not a subtype of itertools.starmap")
            if kwargs and (cls is __class__ or cls.__init__ is __class__.__init__ or cls.__init__ is object.__init__):
                raise TypeError("starmap() takes no keyword arguments")
            if len(args) != 2:
                raise TypeError(f"starmap expected 2 arguments, got {len(args)}")
            return _allocate(__class__, cls, args[0], iter(args[1]))

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                raise TypeError("object.__init__() takes exactly one argument (the instance to initialize)")

        def __iter__(self):
            if not isinstance(self, __class__):
                raise TypeError("expected a starmap instance")
            return self

        def __next__(self):
            return _advance(__class__, self)

    starmap.__qualname__ = "starmap"
    return starmap


starmap = _make_starmap(_itertools._starmap_allocate, _itertools._starmap_next)


def _make_takewhile(_allocate, _state):
    class takewhile:
        """Return successive input items while the predicate is true."""

        __slots__ = ()

        def __new__(cls, /, *args, **kwargs):
            if cls is not __class__:
                if not isinstance(cls, type):
                    raise TypeError(f"itertools.takewhile.__new__(X): X is not a type object ({type(cls).__name__})")
                if not issubclass(cls, __class__):
                    raise TypeError(f"itertools.takewhile.__new__({cls.__name__}): {cls.__name__} is not a subtype of itertools.takewhile")
            if kwargs and (cls is __class__ or cls.__init__ is __class__.__init__ or cls.__init__ is object.__init__):
                raise TypeError("takewhile() takes no keyword arguments")
            if len(args) != 2:
                raise TypeError(f"takewhile expected 2 arguments, got {len(args)}")
            return _allocate(__class__, cls, [args[0], False], args[1])

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                raise TypeError("object.__init__() takes exactly one argument (the instance to initialize)")

        def __iter__(self):
            if not isinstance(self, __class__):
                raise TypeError("expected a takewhile instance")
            return self

        def __next__(self):
            source, state = _state(__class__, self)
            if not state[1]:
                item = next(source)
                if state[0](item):
                    return item
                # Only a false result stops future pulls; exceptions leave the cursor live.
                state[1] = True
            raise StopIteration

    takewhile.__qualname__ = "takewhile"
    return takewhile


takewhile = _make_takewhile(_itertools._predicate_allocate, _itertools._predicate_state)


def _make_dropwhile(_allocate, _state):
    class dropwhile:
        """Drop input items while the predicate is true, then yield all remaining items."""

        __slots__ = ()

        def __new__(cls, /, *args, **kwargs):
            if cls is not __class__:
                if not isinstance(cls, type):
                    raise TypeError(f"itertools.dropwhile.__new__(X): X is not a type object ({type(cls).__name__})")
                if not issubclass(cls, __class__):
                    raise TypeError(f"itertools.dropwhile.__new__({cls.__name__}): {cls.__name__} is not a subtype of itertools.dropwhile")
            if kwargs and (cls is __class__ or cls.__init__ is __class__.__init__ or cls.__init__ is object.__init__):
                raise TypeError("dropwhile() takes no keyword arguments")
            if len(args) != 2:
                raise TypeError(f"dropwhile expected 2 arguments, got {len(args)}")
            return _allocate(__class__, cls, [args[0], False], args[1])

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                raise TypeError("object.__init__() takes exactly one argument (the instance to initialize)")

        def __iter__(self):
            if not isinstance(self, __class__):
                raise TypeError("expected a dropwhile instance")
            return self

        def __next__(self):
            source, state = _state(__class__, self)
            while True:
                item = next(source)
                if state[1]:
                    return item
                if not state[0](item):
                    state[1] = True
                    return item
                del item

    dropwhile.__qualname__ = "dropwhile"
    return dropwhile


dropwhile = _make_dropwhile(_itertools._predicate_allocate, _itertools._predicate_state)


def _make_pairwise(_allocate, _advance):
    class pairwise:
        """Return successive overlapping pairs from the input iterator."""

        __slots__ = ()

        def __new__(cls, /, *args, **kwargs):
            if cls is not __class__:
                if not isinstance(cls, type):
                    raise TypeError(f"itertools.pairwise.__new__(X): X is not a type object ({type(cls).__name__})")
                if not issubclass(cls, __class__):
                    raise TypeError(f"itertools.pairwise.__new__({cls.__name__}): {cls.__name__} is not a subtype of itertools.pairwise")
            if kwargs and (cls is __class__ or cls.__init__ is __class__.__init__ or cls.__init__ is object.__init__):
                raise TypeError("pairwise() takes no keyword arguments")
            if len(args) != 1:
                raise TypeError(f"pairwise expected 1 argument, got {len(args)}")
            return _allocate(__class__, cls, iter(args[0]))

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                raise TypeError("object.__init__() takes exactly one argument (the instance to initialize)")

        def __iter__(self):
            if not isinstance(self, __class__):
                raise TypeError("expected a pairwise instance")
            return self

        def __next__(self):
            return _advance(__class__, self)

    pairwise.__qualname__ = "pairwise"
    return pairwise


pairwise = _make_pairwise(_itertools._pairwise_allocate, _itertools._pairwise_next)


def _make_compress(_allocate, _state):
    class compress:
        """Yield each data item whose corresponding selector is true."""

        __slots__ = ()

        def __new__(cls, /, data, selectors):
            if not isinstance(cls, type) or not issubclass(cls, __class__):
                raise TypeError("expected a compress subclass")
            # Acquire data first. The allocator then obtains the selectors iterator.
            return _allocate(__class__, cls, iter(data), selectors)

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                raise TypeError("object.__init__() takes exactly one argument (the instance to initialize)")

        def __iter__(self):
            if not isinstance(self, __class__):
                raise TypeError("expected a compress instance")
            return self

        def __next__(self):
            selectors, data = _state(__class__, self)
            while True:
                item = next(data)
                if next(selectors):
                    return item
                del item

    compress.__qualname__ = "compress"
    return compress


compress = _make_compress(_itertools._predicate_allocate, _itertools._predicate_state)


class _ZipLongestState:
    __slots__ = ("sources", "active", "fillvalue")

    def __init__(self, iterables, fillvalue):
        self.sources = [iter(source) for source in iterables]
        self.active = len(self.sources)
        self.fillvalue = fillvalue

    def next_row(self):
        if not self.active:
            raise StopIteration
        row = []
        for i, source in enumerate(self.sources):
            if source is None:
                item = self.fillvalue
            else:
                try:
                    item = next(source)
                except StopIteration:
                    if self.active <= 1:
                        self.active = 0
                        # CPython keeps the final exhausted iterator until the zipper dies.
                        break
                    self.active -= 1
                    self.sources[i] = None
                    item = self.fillvalue
                except BaseException:
                    self.active = 0
                    raise
            row.append(item)
        else:
            return tuple(row)
        raise StopIteration


def _make_zip_longest(_allocate, _state, _State):
    class zip_longest:
        """Return a zip_longest object whose .__next__() method returns a tuple where
        the i-th element comes from the i-th iterable argument.  The .__next__()
        method continues until the longest iterable in the argument sequence
        is exhausted and then it raises StopIteration.  When the shorter iterables
        are exhausted, the fillvalue is substituted in their place.  The fillvalue
        defaults to None or can be specified by a keyword argument.
        """

        __slots__ = ()

        def __new__(cls, /, *iterables, **kwargs):
            if cls is not __class__:
                if not isinstance(cls, type):
                    raise TypeError(f"itertools.zip_longest.__new__(X): X is not a type object ({type(cls).__name__})")
                if not issubclass(cls, __class__):
                    raise TypeError(f"itertools.zip_longest.__new__({cls.__name__}): {cls.__name__} is not a subtype of itertools.zip_longest")
            if kwargs and (len(kwargs) != 1 or "fillvalue" not in kwargs):
                raise TypeError("zip_longest() got an unexpected keyword argument")
            return _allocate(__class__, cls, _State(iterables, kwargs.get("fillvalue")))

        def __init__(self, *args, **kwargs):
            if (args or kwargs) and type(self).__init__ is not __class__.__init__:
                raise TypeError("object.__init__() takes exactly one argument (the instance to initialize)")

        def __iter__(self):
            if not isinstance(self, __class__):
                raise TypeError("expected a zip_longest instance")
            return self

        def __next__(self):
            return _state(__class__, self).next_row()

    zip_longest.__qualname__ = "zip_longest"
    return zip_longest


zip_longest = _make_zip_longest(_itertools._zip_longest_allocate, _itertools._zip_longest_state, _ZipLongestState)
del _itertools


class _TeeBlock:
    # Forks only root their current block. Once all forks move past it, its
    # items can be collected without retaining any preceding blocks.
    __slots__ = ("source", "values", "filled", "following", "running")

    def __init__(self, source):
        self.source = source
        self.values = [None] * 57
        self.filled = 0
        self.following = None
        self.running = False


class _tee:
    __slots__ = ("_block", "_index", "__weakref__")

    def __new__(cls, iterable, /):
        if cls is not __class__:
            raise TypeError("expected the itertools._tee class")
        source = iter(iterable)
        fork = object.__new__(cls)
        if type(source) is cls:
            fork._block = source._block
            fork._index = source._index
        else:
            fork._block = _TeeBlock(source)
            fork._index = 0
        return fork

    def __init__(self, *args, **kwargs):
        # Reinitialization leaves the existing cursor in place.
        pass

    def __init_subclass__(cls):
        raise TypeError("type 'itertools._tee' is not an acceptable base type")

    def __copy__(self):
        return type(self)(self)

    def __reduce__(self):
        raise TypeError("cannot pickle '_tee' object")

    def __iter__(self):
        return self

    def __next__(self):
        block = self._block
        index = self._index
        if index == len(block.values):
            if block.following is None:
                block.following = _TeeBlock(block.source)
            self._block = block = block.following
            self._index = index = 0
        if index == block.filled:
            if block.running:
                raise RuntimeError("cannot re-enter the tee iterator")
            block.running = True
            try:
                value = next(block.source)
            except StopIteration:
                pass
            else:
                block.values[index] = value
                block.filled = index + 1
            finally:
                block.running = False
            if index == block.filled:
                raise StopIteration
        self._index = index + 1
        return block.values[index]


def tee(iterable, n=2, /):
    """Return n independent iterators sharing the same input stream."""
    n = _ssize_index(n)
    if n < 0:
        raise ValueError("n must be >= 0")
    if n == 0:
        return ()
    if n > _maxsize // 8:
        raise MemoryError
    forks = [None] * n
    # Both tee() and the first fork acquire an iterator, as in CPython.
    first = _tee(iter(iterable))
    forks[0] = first
    for i in range(1, n):
        forks[i] = _tee(first)
    return tuple(forks)


class _Combination:
    __slots__ = ("_pool", "_indices", "_stopped")

    def __init__(self, iterable, r):
        r = _ssize_index(r)
        self._pool = tuple(iterable)
        if r < 0:
            raise ValueError("r must be non-negative")
        if r > _maxsize // 8:
            raise MemoryError
        if self._with_replacement:
            self._stopped = not self._pool and r != 0
            self._indices = [] if self._stopped else [0] * r
        else:
            self._stopped = r > len(self._pool)
            self._indices = [] if self._stopped else list(range(r))

    def __iter__(self):
        return self

    def __next__(self):
        if self._stopped:
            raise StopIteration
        pool = self._pool
        indices = self._indices
        result = tuple(pool[index] for index in indices)
        width = len(indices)
        for i in range(width - 1, -1, -1):
            limit = len(pool) - 1 if self._with_replacement else len(pool) - width + i
            if indices[i] < limit:
                indices[i] += 1
                for j in range(i + 1, width):
                    indices[j] = indices[i] if self._with_replacement else indices[j - 1] + 1
                return result
        self._stopped = True
        return result


class combinations(_Combination):
    """Yield r-length subsequences of the input, in positional order."""

    __slots__ = ()
    _with_replacement = False


class combinations_with_replacement(_Combination):
    """Yield r-length subsequences, allowing a position to repeat."""

    __slots__ = ()
    _with_replacement = True


_missing = object()


def _ssize_index(value):
    value = _index(value)
    if value > _maxsize or value < -_maxsize - 1:
        raise OverflowError("Python int too large to convert to C ssize_t")
    return value


class product:
    """Cartesian product, advancing the rightmost input first."""

    __slots__ = ("_pools", "_indices", "_stopped")

    def __init__(self, *iterables, repeat=1):
        repeat = _ssize_index(repeat)
        if repeat < 0:
            raise ValueError("repeat argument cannot be negative")
        # Imre targets 64-bit platforms. Each pool needs a Py_ssize_t index.
        if len(iterables) * repeat > _maxsize // 8:
            raise OverflowError("repeat argument too large")
        pools = [tuple(pool) for pool in iterables] if repeat else []
        self._pools = pools * repeat
        self._indices = [0] * len(self._pools)
        self._stopped = any(len(pool) == 0 for pool in pools)

    def __iter__(self):
        return self

    def __next__(self):
        if self._stopped:
            raise StopIteration
        pools = self._pools
        indices = self._indices
        result = tuple(pool[index] for pool, index in zip(pools, indices))
        for position in range(len(indices) - 1, -1, -1):
            indices[position] += 1
            if indices[position] < len(pools[position]):
                return result
            indices[position] = 0
        self._stopped = True
        return result


class batched:
    """Batch data into tuples of length n. The last batch may be shorter than n.

Loops over the input iterable and accumulates data into tuples
up to size n.  The input is consumed lazily, just enough to
fill a batch.  The result is yielded as soon as a batch is full
or when the input iterable is exhausted.

    >>> for batch in batched('ABCDEFG', 3):
    ...     print(batch)
    ...
    ('A', 'B', 'C')
    ('D', 'E', 'F')
    ('G',)

If "strict" is True, raises a ValueError if the final batch is shorter
than n."""

    __slots__ = ()

    def __new__(cls, /, iterable, n, *, strict=False):
        if not issubclass(cls, __class__):
            raise TypeError("expected a batched subclass")
        size = _ssize_index(n)
        strict = bool(strict)
        if size < 1:
            raise ValueError("n must be at least one")
        source = iter(iterable)
        self = object.__new__(cls)
        _batched_state.__set__(self, (source, size, strict))
        return self

    def __init__(self, *args, **kwargs):
        # All state is set by __new__; calling the inherited initializer is inert.
        if (args or kwargs) and type(self).__init__ is not __class__.__init__:
            object.__init__(self, *args, **kwargs)

    def __iter__(self):
        return self

    def __next__(self):
        state = _batched_state.__get__(self)
        if state is None:
            raise StopIteration
        source, size, strict = state
        # Reserve the entire batch before advancing the input.
        items = [None] * size
        for i in range(size):
            try:
                items[i] = next(source)
            except StopIteration:
                break
            except BaseException:
                _batched_state.__set__(self, None)
                raise
        else:
            return tuple(items)
        if i and not strict:
            return tuple(items[:i])
        _batched_state.__set__(self, None)
        if i:
            raise ValueError("batched(): incomplete batch")
        raise StopIteration


class groupby:
    """Group consecutive values with equal keys, sharing one input iterator."""

    __slots__ = ("_source", "_key", "_item", "_target", "_generation")

    def __init__(self, iterable, key=None):
        self._source = iter(iterable)
        self._key = key
        self._item = None
        self._target = _missing
        self._generation = 0

    def __iter__(self):
        return self

    def _advance(self):
        value = next(self._source)
        key = value if self._key is None else self._key(value)
        self._item = key, value

    def __next__(self):
        self._generation += 1
        if self._item is None:
            self._advance()
        while self._target is not _missing:
            key = self._item[0]
            if self._target is not key and not (self._target == key):
                break
            self._advance()
        self._target = self._item[0]
        return self._target, _grouper(self, self._target, self._generation)


class _grouper:
    __slots__ = ("_parent", "_target", "_generation")

    def __init__(self, parent, target, generation):
        self._parent = parent
        self._target = target
        self._generation = generation

    def __iter__(self):
        return self

    def __next__(self):
        parent = self._parent
        if self._generation != parent._generation:
            raise StopIteration
        if parent._item is None:
            parent._advance()
        key = parent._item[0]
        if self._target is not key and not (self._target == key):
            raise StopIteration
        if parent._item is None:
            raise StopIteration
        value = parent._item[1]
        parent._item = None
        return value


class permutations:
    """Yield ordered selections of distinct input positions."""

    __slots__ = ("_pool", "_indices", "_cycles", "_started")

    def __init__(self, iterable, r=None):
        self._pool = tuple(iterable)
        n = len(self._pool)
        if r is None:
            r = n
        elif isinstance(r, int):
            r = _ssize_index(r)
        else:
            raise TypeError("Expected int as r")
        if r < 0:
            raise ValueError("r must be non-negative")
        if r > _maxsize // 8:
            raise MemoryError
        self._started = False
        self._indices = list(range(n))
        self._cycles = list(range(n, n - r, -1)) if r <= n else None

    def __iter__(self):
        return self

    def __next__(self):
        cycles = self._cycles
        if cycles is None:
            raise StopIteration
        indices = self._indices
        if self._started:
            for i in range(len(cycles) - 1, -1, -1):
                cycles[i] -= 1
                if cycles[i] == 0:
                    # Restore the suffix before advancing the preceding position.
                    indices.append(indices.pop(i))
                    cycles[i] = len(indices) - i
                else:
                    j = -cycles[i]
                    indices[i], indices[j] = indices[j], indices[i]
                    break
            else:
                self._cycles = None
                raise StopIteration
        self._started = True
        return tuple(self._pool[indices[i]] for i in range(len(cycles)))


def _islice_index(value, default):
    if value is None:
        return default
    try:
        return _ssize_index(value)
    except BaseException:
        # islice maps failed index conversions to its own argument errors.
        return -1


class islice:
    """Iterate selected positions of an input iterator."""

    __slots__ = ("_source", "_position", "_target", "_stop", "_step")

    def __init__(self, *args, **kwargs):
        # CPython initializes the cursor in its constructor; explicit reinitialization is inert.
        if hasattr(self, "_source"):
            return
        if kwargs:
            raise TypeError("islice() takes no keyword arguments")
        count = len(args)
        if count < 2:
            raise TypeError(f"islice expected at least 2 arguments, got {count}")
        if count > 4:
            raise TypeError(f"islice expected at most 4 arguments, got {count}")
        start = args[1] if count >= 3 else None
        stop = args[2] if count >= 3 else args[1]
        step = args[3] if count == 4 else None
        target = _islice_index(start, 0)
        limit = _islice_index(stop, -1)
        if stop is not None and limit == -1:
            raise ValueError("Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize.")
        if target < 0 or limit < -1:
            raise ValueError("Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.")
        stride = _islice_index(step, 1)
        if stride < 1:
            raise ValueError("Step for islice() must be a positive integer or None.")
        self._source = iter(args[0])
        self._position = 0
        self._target = target
        self._stop = limit
        self._step = stride

    def __iter__(self):
        return self

    def __next__(self):
        source = self._source
        if source is None:
            raise StopIteration
        try:
            while self._position < self._target:
                next(source)
                self._position += 1
            if self._stop != -1 and self._position >= self._stop:
                raise StopIteration
            value = next(source)
        except BaseException:
            self._source = None
            raise
        self._position += 1
        target = self._target + self._step
        if target > _maxsize or (self._stop != -1 and target > self._stop):
            target = self._stop
        self._target = target
        return value
