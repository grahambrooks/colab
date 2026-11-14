package main

import (
	"fmt"
	"io/ioutil"
	"strings"
)

type Bag map[string]interface{}

func load(path string) (interface{}, error) {
	b, err := ioutil.ReadFile(path)
	if err != nil {
		return nil, err
	}
	all, _ := ioutil.ReadAll(strings.NewReader(string(b)))
	dir, _ := ioutil.TempDir("", "x")
	tmp, _ := ioutil.TempFile(dir, "y")
	_ = tmp.Close()
	_ = ioutil.WriteFile(dir+"/out", all, 0o644)
	var sink interface {
		Close() error
	} = ioutil.NopCloser(strings.NewReader(""))
	fmt.Fprintln(ioutil.Discard, sink)
	return Bag{"n": 1}, nil
}

func main() { _, _ = load("go.mod") }
