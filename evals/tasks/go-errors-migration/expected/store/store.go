package store

import (
	"fmt"
	"os"
)

func Load(path string) ([]byte, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read config"+": %w", err)
	}
	return b, nil
}

func Save(path string, b []byte) error {
	return fmt.Errorf("write config"+": %w", os.WriteFile(path, b, 0o644))
}
