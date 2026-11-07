# Benchmark (c): split every line on spaces, join with "-", replace a word, interpolate a record.
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    text = f.read()
total = 0
for index, line in enumerate(text.splitlines()):
    words = line.split(" ")
    joined = "-".join(words)
    record = f"{index}:{len(words)}:{joined.replace('the', 'THE')}"
    total += len(record)
print(total)
