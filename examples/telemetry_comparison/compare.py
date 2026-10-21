from .model import Sample, Summary


def distance(expected: int, actual: int) -> int:
    delta = actual - expected
    return -delta if delta < 0 else delta


def square_error(expected: int, actual: int) -> int:
    delta = actual - expected
    return delta * delta


def alert(reference: list[Sample], observed: list[Sample], threshold: int) -> bool:
    return any(
        distance(expected.value, actual.value) > threshold
        for expected, actual in zip(reference, observed, strict=True)
    )


def within(reference: list[Sample], observed: list[Sample], threshold: int) -> bool:
    return all(
        distance(expected.value, actual.value) <= threshold
        for expected, actual in zip(reference, observed, strict=True)
    )


def latest(
    reference: list[Sample], observed: list[Sample], threshold: int
) -> int | None:
    for expected, actual in zip(reversed(reference), reversed(observed), strict=True):
        if distance(expected.value, actual.value) > threshold:
            return expected.timestamp
    return None


def summarize(
    reference: list[Sample], observed: list[Sample], threshold: int
) -> Summary:
    signed_error = sum(
        actual.value - expected.value
        for expected, actual in zip(reference, observed, strict=True)
    )
    absolute_error = sum(
        distance(expected.value, actual.value)
        for expected, actual in zip(reference, observed, strict=True)
    )
    squared_error = sum(
        square_error(expected.value, actual.value)
        for expected, actual in zip(reference, observed, strict=True)
    )
    return Summary(
        len(reference),
        signed_error,
        absolute_error,
        squared_error,
        alert(reference, observed, threshold),
        within(reference, observed, threshold),
        latest(reference, observed, threshold),
    )
