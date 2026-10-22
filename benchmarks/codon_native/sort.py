"""Native eager-key sorting boundary for the shared application algorithms."""


def sort_with_key(iterable, key, reverse: bool = False):
    """Materialize input, compute each key once, and preserve source-order ties."""
    values = list(iterable)
    decorated = [(key(value), index, value) for index, value in enumerate(values)]
    decorated.sort(
        key=lambda entry: (entry[0], -entry[1] if reverse else entry[1]),
        reverse=reverse,
    )
    return [entry[2] for entry in decorated]
