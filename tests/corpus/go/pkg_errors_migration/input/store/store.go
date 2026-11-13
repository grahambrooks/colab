package store

import (
	"os"

	"github.com/pkg/errors"
)

// Open wraps failures with errors.Wrap(err, "...") — this comment stays.
func Open(path string) (*os.File, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, errors.Wrap(errors.Wrap(err, "open"), "store")
	}
	return f, nil
}

func Remove(path string, attempt int) error {
	if err := os.Remove(path); err != nil {
		return errors.Wrapf(err, "remove %s (attempt %d)", path, attempt)
	}
	return nil
}
