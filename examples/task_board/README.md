# Task board

A small browser application backed by a JSON Lines journal. Add a task, mark it complete, refresh, or restart the process: the journal preserves the tasks and the next ID. All routes, JSON validation, persistence and HTTP handling are ordinary Python. The same source runs on CPython or compiles to an Imre executable.

## Run locally

```sh
python3 examples/task_board/app.py 8080 /tmp/tasks.jsonl
```

Open <http://127.0.0.1:8080/>. To use Imre, build the compiler and runtime in release mode and point the compiler at the application's module root:

```sh
cargo build --release -p imre-compiler -p imre-runtime
target/release/imre build examples/task_board/app.py --module-root examples/task_board \
  --runtime target/release/libimre_runtime.a --opt-level 3 -o /tmp/task-board
/tmp/task-board 8080 /tmp/tasks.jsonl
```

The service binds to IPv4 loopback. The journal is for a single writer: each acknowledged mutation appends and closes a file before changing the visible state. There is no database, multi-process locking or fsync guarantee. The listener uses public Python socket and poll APIs, tracks partial reads/writes, and caps headers at 8 KiB and request bodies at 16 KiB. It accepts HTTP/1.1 content lengths; it does not implement chunked requests, TLS, authentication, WSGI, or Flask extensions.

## Routes and views

| Request | Result |
|---|---|
| GET / | Browser form and task list |
| GET /tasks | JSON list |
| POST /tasks | Create a task from a JSON object with a title; returns 201 |
| GET /tasks/<id> | JSON for one task; missing IDs return 404 |
| POST /tasks/<id>/done | Complete the task, idempotently |

[tinyweb.py](tinyweb.py) matches literal path segments and one integer ID, then invokes a class-based view. [httpwire.py](httpwire.py) deals with connections and framing. Application code in [board.py](board.py) has no socket calls. Registration is explicit:

```python
class TaskDetail(BoardView):
    def handle(self, req: Request) -> Response:
        if req.id < 1 or req.id > len(self.board.tasks):
            return error(404, "task not found")
        return json_response(200, task_json(self.board.tasks[req.id - 1], req.id))

app.get("/tasks/<int:id>", TaskDetail(board))
```

Views return a typed response. The dynamic ID is available on the request; application state belongs to the view object. This avoids the function registries and ambient request context used by Flask.

## Verify

```sh
python3 examples/task_board/verify.py --python python3
python3 examples/task_board/verify.py --binary /tmp/task-board
```

The verifier makes real requests. It checks Unicode, JSON errors, content types, HEAD and Allow, idempotence, fragmented and pipelined request bodies, a concurrent client, framing limits, and journal recovery after restarting the server.
