# Benchmark (b), the language's own JSON library (C in CPython).
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    value = json.load(f)
print(len(json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()))
