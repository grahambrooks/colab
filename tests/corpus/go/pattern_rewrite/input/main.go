package main

import (
	"fmt"
	"log"
	"os"

	"github.com/pkg/errors"
)

func load(path string) ([]byte, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, errors.Wrap(err, "read config")
	}
	log.Println("DEBUG", "loaded", len(data))
	return data, nil
}

func main() {
	if _, err := load("app.toml"); err != nil {
		fmt.Println(errors.Wrap(err, "startup"))
		log.Println("fatal:", err)
	}
}
