// Benchmark (d): naive recursion and an arithmetic loop.
function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}

let total = 0;
for (let i = 0; i < 10000000; i++) total = total + (i * i) % 7;
console.log(`${fib(30)} ${total}`);
