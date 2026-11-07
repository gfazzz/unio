// Benchmark (c): split every line on spaces, join with "-", replace a word, interpolate a record.
package main

import (
	"fmt"
	"os"
	"strings"
	"unicode/utf8"
)

func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	lines := strings.Split(string(data), "\n")
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	total := 0
	for index, line := range lines {
		words := strings.Split(line, " ")
		joined := strings.Join(words, "-")
		record := fmt.Sprintf("%d:%d:%s", index, len(words), strings.ReplaceAll(joined, "the", "THE"))
		total += utf8.RuneCountInString(record)
	}
	fmt.Println(total)
}
