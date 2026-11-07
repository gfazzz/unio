# Benchmark (d): naive recursion and an arithmetic loop.
def fib(n)
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end

total = 0
(0...10_000_000).each { |i| total = total + i * i % 7 }
puts "#{fib(30)} #{total}"
