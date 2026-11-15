def update(pyproject, requirements):
    pyproject["project"]["dependencies"] += [str(requirement) for requirement in requirements]


components = ["already"]
project = {"project": {"dependencies": components}}
update(project, ["idna", "ruff"])
print(components is project["project"]["dependencies"], components)
