package store

import (
	"fmt"
	"os"
)

// Open wraps failures with errors.Wrap(err, "...") — this comment stays.
func Open(path string) (*os.File, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", "store", fmt.Errorf("%s: %w", "open", err))
	}
	return f, nil
}

func Remove(path string, attempt int) error {
	if err := os.Remove(path); err != nil {
		return fmt.Errorf("remove %s (attempt %d)"+": %w", path, attempt, err)
	}
	return nil
}
