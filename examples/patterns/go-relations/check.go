package files

import "os"

func check(p string) error {
	_, err := os.Stat(p)
	if err != nil {
		return err
	}
	n := len(p)
	_ = n
	if err != nil {
		return err
	}
	return nil
}
