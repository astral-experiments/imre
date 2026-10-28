import sys
from flask import Flask
from flask.frozen.export import export_application
import catalog_views

app = Flask("catalog", static_folder=None)
app.config["SESSION_COOKIE_NAME"] = "catalog"
app.get("/items/<label>")(catalog_views.card)
app.add_url_rule("/items/stock", view_func=catalog_views.stock, methods=["GET", "PUT"])
export_application(app, {"card": "<article>{{ detail }} — {{label}} / {{label}}</article>"}, sys.argv[1])
