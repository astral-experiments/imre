def render(template: str, values: dict[str, str]) -> str:
    return template.format_map(values)


print(render("{ty} check {paths}", {"ty": "ty", "paths": "src tests"}))
try:
    render("{missing}", {})
except KeyError as error:
    print(error.args)
