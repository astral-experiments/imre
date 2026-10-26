"""Prompts in one case share an interactive namespace.

    >>> values = [3]
    >>> values.append(7)
    >>> values
    [3, 7]
    >>> for value in values:
    ...     print(value)
    ...
    3
    7
    >>> saved = 9
    >>> saved = fails()
    Traceback (most recent call last):
      ...
    ValueError: deliberate
    >>> saved
    9
    >>> fresh = fails()
    Traceback (most recent call last):
      ...
    ValueError: deliberate
    >>> fresh
    Traceback (most recent call last):
      ...
    NameError: name 'fresh' is not defined
    >>> print("left   right")  # doctest: +NORMALIZE_WHITESPACE
    left right
    >>> print("prefix-middle-suffix")  # doctest: +ELLIPSIS
    prefix...suffix
    >>> print('# doctest: +SKIP')
    # doctest: +SKIP
    >>> print("# doctest: +UNKNOWN")  # doctest: +ELLIPSIS
    # doctest: +...
    >>> raise ValueError("should not run")  # doctest: +SKIP
"""


def fails() -> int:
    raise ValueError("deliberate")


def twice(n: int) -> int:
    """The module's own globals are visible in a function docstring.

    >>> twice(4)
    8
    """
    return n + n
