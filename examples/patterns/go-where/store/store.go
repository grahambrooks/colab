package store

import "github.com/pkg/errors"

func Save(db DB, v Value) error {
	if err := db.Put(v); err != nil {
		return errors.Wrap(err, "put")
	}
	return errors.Wrap(db.Sync(), "sync")
}
