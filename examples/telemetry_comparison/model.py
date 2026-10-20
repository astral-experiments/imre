class Sample:
    timestamp: int
    value: int

    def __init__(self, timestamp: int, value: int) -> None:
        self.timestamp = timestamp
        self.value = value


class Summary:
    samples: int
    signed_error: int
    absolute_error: int
    squared_error: int
    alert: bool
    within: bool
    latest: int | None

    def __init__(
        self,
        samples: int,
        signed_error: int,
        absolute_error: int,
        squared_error: int,
        alert: bool,
        within: bool,
        latest: int | None,
    ) -> None:
        self.samples = samples
        self.signed_error = signed_error
        self.absolute_error = absolute_error
        self.squared_error = squared_error
        self.alert = alert
        self.within = within
        self.latest = latest
