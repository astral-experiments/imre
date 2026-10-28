"""Register and freeze a typed Flask application for the Imre demo."""

from flask import Flask
from flask.frozen.export import export_application
from jinja2.frozen_rows import write_renderer

__all__ = ["Flask", "export_application", "write_renderer"]
