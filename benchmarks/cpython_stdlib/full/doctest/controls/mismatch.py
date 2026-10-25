"""An ordinary failure runs the remaining prompts; debug stops immediately.

    >>> 3
    4
    >>> record()
"""

visits: list[int] = []


def record() -> None:
    visits.append(1)
