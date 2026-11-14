package store

import (
	"os"

	"github.com/pkg/errors"
)

func Load(path string) ([]byte, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, errors.Wrap(err, "read config")
	}
	return b, nil
}

func Save(path string, b []byte) error {
	return errors.Wrap(os.WriteFile(path, b, 0o644), "write config")
}
