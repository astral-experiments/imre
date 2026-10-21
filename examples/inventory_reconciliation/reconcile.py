from .model import Fulfillment, Inventory, Report, Request, Stock


def fulfill(request: Request, *, inventory: Inventory) -> Fulfillment:
    available = inventory.remaining.get(request.sku)
    quantity = request.quantity
    # Missing or empty stock fills nothing; min is outside the native subset.
    filled = (available and (available if available < quantity else quantity)) or 0  # noqa: FURB136
    if available is not None:
        inventory.remaining[request.sku] = available - filled
    return Fulfillment(request, filled=filled, shortage=request.quantity - filled)


def reconcile(stocks: list[Stock], *, batches: list[list[Request]]) -> Report:
    remaining: dict[str, int] = {stock.sku: stock.quantity for stock in stocks}
    inventory = Inventory(remaining=remaining)
    positive: list[Request] = [
        request for batch in batches for request in batch if request.quantity > 0
    ]
    # A dict preserves first-request order while repeated values start at zero.
    requested: dict[str, int] = {request.sku: 0 for request in positive}
    requested_skus: set[str] = {*requested}
    for request in positive:
        requested[request.sku] += request.quantity
    # Eagerness and source order matter: every call mutates the same inventory.
    results: list[Fulfillment] = [
        fulfill(request, inventory=inventory) for request in positive
    ]
    request_rows = 0
    for batch in batches:
        request_rows += len(batch)
    return Report(
        stock_rows=len(stocks),
        batches=len(batches),
        request_rows=request_rows,
        results=results,
        requested=requested,
        requested_skus=requested_skus,
        remaining=remaining,
    )
