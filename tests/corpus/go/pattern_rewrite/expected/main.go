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
		return nil, fmt.Errorf("%s: %w", "read config", err)
	}
	return data, nil
}

func main() {
	if _, err := load("app.toml"); err != nil {
		fmt.Println(fmt.Errorf("%s: %w", "startup", err))
		log.Println("fatal:", err)
	}
}
