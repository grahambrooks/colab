package files

import (
	"fmt"
	"os"
)

func check(p string) error {
	_, err := os.Stat(p)
	if err != nil {
		return fmt.Errorf("stat: %w", err)
	}
	n := len(p)
	_ = n
	if err != nil {
		return err
	}
	return nil
}
