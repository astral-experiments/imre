import sys
from mixed_wsgi import app
from werkzeug.serving import make_server

server = make_server("127.0.0.1", 0, app)
with open(sys.argv[1], "w") as ready:
    ready.write(str(server.server_port))
server.serve_forever()
