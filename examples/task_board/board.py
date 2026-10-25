"""A task board with an append-only JSON journal. The HTTP layer is in tinyweb."""

import json
from tinyweb import Request, Response, Router, View, error, json_response


class Task:
    title: str
    done: bool

    def __init__(self, title: str) -> None:
        self.title = title
        self.done = False


def task_json(task: Task, ident: int) -> str:
    done = "true" if task.done else "false"
    title = json.dumps(task.title, ensure_ascii=False)
    return f'{{"id":{ident},"title":{title},"done":{done}}}'


class Board:
    path: str
    tasks: list[Task]

    def __init__(self, path: str) -> None:
        self.path = path
        self.tasks = []

    def load(self) -> None:
        path = self.path
        with open(path, "ab") as _log:
            pass
        with open(path, encoding="utf-8") as saved:
            for line in saved:
                entry = json.loads(line)
                op: str = entry[0]
                arg: str = entry[1]
                if op == "add":
                    self.tasks.append(Task(arg))
                elif op == "done" and 1 <= int(arg) <= len(self.tasks):
                    self.tasks[int(arg) - 1].done = True
                else:
                    raise ValueError("invalid task journal entry")

    def record(self, op: str, arg: str) -> None:
        with open(self.path, "ab") as stream:
            stream.write(
                (json.dumps([op, arg], ensure_ascii=False) + "\n").encode("utf-8")
            )


class BoardView(View):
    board: Board

    def __init__(self, board: Board) -> None:
        self.board = board


class TaskList(BoardView):
    def handle(self, req: Request) -> Response:
        rows: list[str] = []
        for i in range(len(self.board.tasks)):
            rows.append(task_json(self.board.tasks[i], i + 1))
        return json_response(200, "[" + ",".join(rows) + "]")


class TaskDetail(BoardView):
    def handle(self, req: Request) -> Response:
        if req.id < 1 or req.id > len(self.board.tasks):
            return error(404, "task not found")
        return json_response(200, task_json(self.board.tasks[req.id - 1], req.id))


def valid_title(value: str) -> str:
    if not isinstance(value, str):
        raise ValueError("expected a title")
    return value.strip()


class CreateTask(BoardView):
    def handle(self, req: Request) -> Response:
        if req.media != "application/json":
            return error(415, "expected application/json")
        try:
            parsed = json.loads(req.body.decode("utf-8"))
            title = valid_title(parsed["title"])
        except (ValueError, TypeError, KeyError, IndexError):
            return error(400, "expected a title")
        if not title or len(title) > 200:
            return error(400, "title must be 1 to 200 characters")
        self.board.record("add", title)
        task = Task(title)
        self.board.tasks.append(task)
        return json_response(201, task_json(task, len(self.board.tasks)))


class CompleteTask(BoardView):
    def handle(self, req: Request) -> Response:
        if req.id < 1 or req.id > len(self.board.tasks):
            return error(404, "task not found")
        task = self.board.tasks[req.id - 1]
        if not task.done:
            self.board.record("done", str(req.id))
            task.done = True
        return json_response(200, task_json(task, req.id))


class Home(View):
    def handle(self, req: Request) -> Response:
        return Response(200, "text/html; charset=utf-8", PAGE.encode("utf-8"))


def make_app(path: str) -> Router:
    board = Board(path)
    board.load()
    app = Router()
    app.get("/", Home())
    app.get("/tasks", TaskList(board))
    app.post("/tasks", CreateTask(board))
    app.get("/tasks/<int:id>", TaskDetail(board))
    app.post("/tasks/<int:id>/done", CompleteTask(board))
    return app


PAGE = """<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Task board</title><style>body{font:1rem/1.5 system-ui;max-width:38rem;margin:3rem auto;padding:0 1rem}li{margin:.6rem 0}button{margin-left:.5rem}</style>
<main><h1>Task board</h1><form id="new"><label>New task <input id="title" required maxlength="200"></label><button>Add task</button></form><p role="status" id="message"></p><ul id="tasks"></ul></main>
<script>
const list=document.querySelector('#tasks'), message=document.querySelector('#message'), title=document.querySelector('#title');
async function request(path,options){const r=await fetch(path,options),data=await r.json();if(!r.ok)throw Error(data.error);return data}
async function reload(){const rows=await request('/tasks');list.replaceChildren();for(const row of rows){const li=document.createElement('li');li.textContent=row.title;if(row.done){li.style.textDecoration='line-through'}else{const btn=document.createElement('button');btn.textContent='Complete';btn.onclick=async()=>{try{await request('/tasks/'+row.id+'/done',{method:'POST'});await reload()}catch(e){message.textContent=e.message}};li.append(btn)}list.append(li)}}
document.querySelector('#new').onsubmit=async e=>{e.preventDefault();try{await request('/tasks',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({title:title.value})});title.value='';await reload();message.textContent='Task added'}catch(e){message.textContent=e.message}};
reload().catch(e=>message.textContent=e.message);
</script></html>"""
