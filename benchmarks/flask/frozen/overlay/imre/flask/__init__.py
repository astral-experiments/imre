"""Register and freeze a typed Flask application for the Imre demo."""

from flask import Flask
from flask.frozen.export import export_application

__all__ = ["Flask", "export_application"]
