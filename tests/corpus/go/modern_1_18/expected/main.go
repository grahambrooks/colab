package main

import (
	"fmt"
	"io"
	"os"
	"strings"
)

type Bag map[string]any

func load(path string) (any, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	all, _ := io.ReadAll(strings.NewReader(string(b)))
	dir, _ := os.MkdirTemp("", "x")
	tmp, _ := os.CreateTemp(dir, "y")
	_ = tmp.Close()
	_ = os.WriteFile(dir+"/out", all, 0o644)
	var sink interface {
		Close() error
	} = io.NopCloser(strings.NewReader(""))
	fmt.Fprintln(io.Discard, sink)
	return Bag{"n": 1}, nil
}

func main() { _, _ = load("go.mod") }
