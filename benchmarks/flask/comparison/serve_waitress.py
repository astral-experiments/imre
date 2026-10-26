"""Run the same Flask application with one Waitress application worker."""
import sys
from pathlib import Path

from tiny_flask_app import app
from waitress import create_server

server = create_server(app, host="127.0.0.1", port=0, threads=1)
Path(sys.argv[1]).write_text(str(server.effective_port))
server.run()
