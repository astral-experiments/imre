from .model import Fulfillment, Inventory, Report, Request, Stock


def fulfill(request: Request, inventory: Inventory) -> Fulfillment:
    filled = 0
    if request.sku in inventory.remaining:
        available = inventory.remaining[request.sku]
        filled = request.quantity
        if available < filled:  # noqa: PLR1730 -- min is outside the native subset.
            filled = available
        inventory.remaining[request.sku] = available - filled
    return Fulfillment(request, filled, request.quantity - filled)


def reconcile(stocks: list[Stock], batches: list[list[Request]]) -> Report:
    remaining: dict[str, int] = {stock.sku: stock.quantity for stock in stocks}
    inventory = Inventory(remaining)
    positive: list[Request] = [
        request for batch in batches for request in batch if request.quantity > 0
    ]
    requested_skus: set[str] = {request.sku for request in positive}
    # A dict preserves first-request order while repeated values start at zero.
    requested: dict[str, int] = {request.sku: 0 for request in positive}
    for request in positive:
        requested[request.sku] += request.quantity
    # Eagerness and source order matter: every call mutates the same inventory.
    results: list[Fulfillment] = [fulfill(request, inventory) for request in positive]
    request_rows = 0
    for batch in batches:
        request_rows += len(batch)
    return Report(
        len(stocks),
        len(batches),
        request_rows,
        results,
        requested,
        requested_skus,
        remaining,
    )
