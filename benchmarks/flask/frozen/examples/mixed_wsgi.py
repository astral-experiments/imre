from flask import Flask, request, session

app = Flask(__name__, static_folder=None)
app.config["SECRET_KEY"] = "local-probe-key"
greeting = app.jinja_env.compile_text("Hello, {{ name }}{{ suffix }}")


@app.get("/health")
def health():
    return {"ok": True}


@app.get("/hello/<name>")
def hello(name: str):
    return greeting.render({"name": name, "suffix": request.args.get("suffix", "!")})


@app.post("/total")
def total():
    values = request.get_json()["values"]
    return {"count": len(values), "total": sum(values)}, 201


@app.get("/visits")
def visits():
    n = session.get("visits", 0) + 1
    session["visits"] = n
    return {"visits": n}


app.freeze_routes()
