from flask.frozen.state import AppState, json_bools, json_ints
from werkzeug.wrappers.frozen import Request, Response


def health(req: Request, capture: str, state: AppState) -> Response:
    return json_bools({"ok": True}, 200)


def hello(req: Request, capture: str, state: AppState) -> Response:
    return state.render("greeting", {"name": capture, "suffix": req.arg("suffix", "!")})


def total(req: Request, capture: str, state: AppState) -> Response:
    values = req.integer_lists()["values"]
    return json_ints({"count": len(values), "total": sum(values)}, 201)


def visits(req: Request, capture: str, state: AppState) -> Response:
    session = state.session(req)
    session["visits"] = session.get("visits", 0) + 1
    out = json_ints(session, 200)
    state.save(session, out)
    return out
