// Benchmark (d): naive recursion and an arithmetic loop.
package main

import "fmt"

func fib(n int64) int64 {
	if n < 2 {
		return n
	}
	return fib(n-1) + fib(n-2)
}

func main() {
	var total int64
	for i := int64(0); i < 10000000; i++ {
		total = total + i*i%7
	}
	fmt.Printf("%d %d\n", fib(30), total)
}
