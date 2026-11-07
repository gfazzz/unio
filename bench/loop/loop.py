# Benchmark (d): naive recursion and an arithmetic loop.
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

total = 0
for i in range(0, 10000000):
    total = total + i * i % 7
print(f"{fib(30)} {total}")
