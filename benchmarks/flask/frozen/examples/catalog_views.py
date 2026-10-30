from imre.flask.views import AppState, Request, Response, json_ints


def card(req: Request, capture: str, state: AppState) -> Response:
    return state.render("card", {"label": capture, "detail": req.arg("q", "?")})


def stock(req: Request, capture: str, state: AppState) -> Response:
    ledger = state.session(req)
    ledger["inventory"] = ledger.get("inventory", 100) - 3
    out = json_ints(ledger, 200)
    state.save(ledger, out)
    return out
