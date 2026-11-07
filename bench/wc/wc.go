// Benchmark (a): lines, words, and UTF-8 bytes of a large text.
package main

import (
	"fmt"
	"os"
	"strings"
)

func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	text := string(data)
	fmt.Printf("%d %d %d\n", strings.Count(text, "\n"), len(strings.Fields(text)), len(data))
}
