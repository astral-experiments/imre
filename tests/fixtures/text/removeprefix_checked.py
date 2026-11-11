def node_name(line: str) -> str:
    return line.split("(")[1].split(")")[0].split("::")[-1].removeprefix("&'a ")


print(node_name("ModModule(&'a crate::ModModule),"))
print(node_name("Node(&'a Identifier),"))
