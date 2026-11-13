package store

import "fmt"

func Save(db DB, v Value) error {
	if err := db.Put(v); err != nil {
		return fmt.Errorf("put"+": %w", err)
	}
	return fmt.Errorf("sync"+": %w", db.Sync())
}
