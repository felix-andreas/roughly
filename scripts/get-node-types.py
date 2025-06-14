from pathlib import Path
import json

# data = json.loads(Path(__file__).parent.read_text())
data = json.loads((Path.home() / "Downloads/node-types.json").read_text())

for x in data:
    kind = f'{x["type"]}'.replace("\\", "\\\\").replace('"', '\\"')
    print(f'"{kind}" => "{kind}",')
print("_ => unreachable!(),")

# named
print("------NAMED------")
for x in data:
    if not x["named"]:
        continue
    kind = f'{x["type"]}'.replace("\\", "\\\\").replace('"', '\\"')
    print(f'"{kind}" => "{kind}",')
print("_ => unreachable!(),")


# unamed
print("------UNNAMED------")
for x in data:
    if x["named"]:
        continue
    kind = f'{x["type"]}'.replace("\\", "\\\\").replace('"', '\\"')
    print(f'"{kind}" => "{kind}",')
print("_ => unreachable!(),")
