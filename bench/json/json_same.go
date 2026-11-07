// Benchmark (b), same algorithm as stdlib/alloc/json.unio.
package main

import (
	"fmt"
	"os"
	"strconv"
	"strings"
)

type Field struct {
	Key   string
	Value any
}

type Object []Field

type Reader struct {
	chars    []rune
	position int
}

func (r *Reader) peek() rune {
	if r.position < len(r.chars) {
		return r.chars[r.position]
	}
	return 0
}

func (r *Reader) advance() rune {
	c := r.peek()
	r.position++
	return c
}

func skipSpace(r *Reader) {
	for c := r.peek(); c == ' ' || c == '\t' || c == '\n' || c == '\r'; c = r.peek() {
		r.advance()
	}
}

func expect(r *Reader, word string) {
	for _, c := range word {
		if r.advance() != c {
			panic("expected " + word)
		}
	}
}

func readValue(r *Reader) any {
	skipSpace(r)
	switch r.peek() {
	case '{':
		return readObject(r)
	case '[':
		return readArray(r)
	case '"':
		return readText(r)
	case 't':
		expect(r, "true")
		return true
	case 'f':
		expect(r, "false")
		return false
	case 'n':
		expect(r, "null")
		return nil
	}
	return readNumber(r)
}

func readObject(r *Reader) any {
	r.advance()
	fields := Object{}
	skipSpace(r)
	if r.peek() == '}' {
		r.advance()
		return fields
	}
	for {
		skipSpace(r)
		key := readText(r)
		skipSpace(r)
		expect(r, ":")
		fields = append(fields, Field{key, readValue(r)})
		skipSpace(r)
		if r.advance() == '}' {
			return fields
		}
	}
}

func readArray(r *Reader) any {
	r.advance()
	items := []any{}
	skipSpace(r)
	if r.peek() == ']' {
		r.advance()
		return items
	}
	for {
		items = append(items, readValue(r))
		skipSpace(r)
		if r.advance() == ']' {
			return items
		}
	}
}

var escapes = map[rune]rune{'"': '"', '\\': '\\', '/': '/', 'n': '\n', 'r': '\r', 't': '\t', 'b': '\b', 'f': '\f'}

func readText(r *Reader) string {
	r.advance()
	var result strings.Builder
	for {
		// A run of characters that need no escape is copied at once.
		start := r.position
		for r.position < len(r.chars) {
			c := r.chars[r.position]
			if c == '"' || c == '\\' || c < ' ' {
				break
			}
			r.position++
		}
		if r.position > start {
			result.WriteString(string(r.chars[start:r.position]))
		}
		c := r.advance()
		if c == '"' {
			return result.String()
		}
		if c == '\\' {
			e := r.advance()
			if e == 'u' {
				hex := string([]rune{r.advance(), r.advance(), r.advance(), r.advance()})
				code, _ := strconv.ParseInt(hex, 16, 32)
				result.WriteRune(rune(code))
			} else {
				result.WriteRune(escapes[e])
			}
		} else {
			result.WriteRune(c)
		}
	}
}

func readNumber(r *Reader) any {
	var text strings.Builder
	for c := r.peek(); strings.ContainsRune("-+.eE", c) || (c >= '0' && c <= '9'); c = r.peek() {
		text.WriteRune(r.advance())
	}
	s := text.String()
	if strings.ContainsAny(s, ".eE") {
		f, _ := strconv.ParseFloat(s, 64)
		return f
	}
	i, _ := strconv.ParseInt(s, 10, 64)
	return i
}

func escape(c rune) string {
	switch c {
	case '"':
		return "\\\""
	case '\\':
		return "\\\\"
	case '\n':
		return "\\n"
	case '\r':
		return "\\r"
	case '\t':
		return "\\t"
	}
	if c < 0x20 {
		return fmt.Sprintf("\\u%04x", c)
	}
	return string(c)
}

func quote(text string) string {
	result := "\""
	for _, c := range text {
		result += escape(c)
	}
	return result + "\""
}

func writeValue(v any) string {
	switch v := v.(type) {
	case nil:
		return "null"
	case bool:
		return strconv.FormatBool(v)
	case int64:
		return strconv.FormatInt(v, 10)
	case float64:
		return strconv.FormatFloat(v, 'g', -1, 64)
	case string:
		return quote(v)
	case []any:
		parts := make([]string, len(v))
		for i, item := range v {
			parts[i] = writeValue(item)
		}
		return "[" + strings.Join(parts, ",") + "]"
	case Object:
		parts := make([]string, len(v))
		for i, f := range v {
			parts[i] = quote(f.Key) + ":" + writeValue(f.Value)
		}
		return "{" + strings.Join(parts, ",") + "}"
	}
	panic("unknown value")
}

func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	r := &Reader{chars: []rune(string(data))}
	fmt.Println(len(writeValue(readValue(r))))
}
