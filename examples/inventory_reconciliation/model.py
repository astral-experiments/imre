class Stock:
    sku: str
    quantity: int

    def __init__(self, sku: str, *, quantity: int) -> None:
        self.sku = sku
        self.quantity = quantity


class Request:
    batch: int
    row: int
    sku: str
    quantity: int

    def __init__(self, *, batch: int, row: int, sku: str, quantity: int) -> None:
        self.batch = batch
        self.row = row
        self.sku = sku
        self.quantity = quantity


class Inventory:
    remaining: dict[str, int]

    def __init__(self, remaining: dict[str, int]) -> None:
        self.remaining = remaining


class Fulfillment:
    request: Request
    filled: int
    shortage: int

    def __init__(self, request: Request, *, filled: int, shortage: int) -> None:
        self.request = request
        self.filled = filled
        self.shortage = shortage


class Report:
    stock_rows: int
    batches: int
    request_rows: int
    results: list[Fulfillment]
    requested: dict[str, int]
    requested_skus: set[str]
    remaining: dict[str, int]

    def __init__(
        self,
        *,
        stock_rows: int,
        batches: int,
        request_rows: int,
        results: list[Fulfillment],
        requested: dict[str, int],
        requested_skus: set[str],
        remaining: dict[str, int],
    ) -> None:
        self.stock_rows = stock_rows
        self.batches = batches
        self.request_rows = request_rows
        self.results = results
        self.requested = requested
        self.requested_skus = requested_skus
        self.remaining = remaining
