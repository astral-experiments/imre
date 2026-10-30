import sys
from imre.flask import Flask, export_application
import mixed_views

app = Flask("mixed", static_folder=None)
app.get("/health")(mixed_views.health)
app.get("/hello/<name>")(mixed_views.hello)
app.post("/total")(mixed_views.total)
app.get("/visits")(mixed_views.visits)
export_application(app, {"greeting": "Hello, {{ name }}{{ suffix }}"}, sys.argv[1])
