from flask import Flask, render_template_string, request, session

app = Flask(__name__)
app.config["SECRET_KEY"] = "local-probe-key"


@app.get("/health")
def health():
    return {"ok": True}


@app.get("/hello/<name>")
def hello(name):
    return render_template_string(
        "Hello, {{ name }}{{ suffix }}",
        name=name,
        suffix=request.args.get("suffix", "!"),
    )


@app.post("/total")
def total():
    values = request.get_json()["values"]
    return {"count": len(values), "total": sum(values)}, 201


@app.get("/visits")
def visits():
    count = session.get("visits", 0) + 1
    session["visits"] = count
    return {"visits": count}
