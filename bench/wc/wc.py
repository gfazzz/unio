# Benchmark (a): lines, words, and UTF-8 bytes of a large text.
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    text = f.read()
print(f"{text.count(chr(10))} {len(text.split())} {len(text.encode())}")
