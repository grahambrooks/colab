package b

import (
	"fmt"
	"log"

	"github.com/pkg/errors"
)

func load() error {
	log.Print("x", 1)
	fmt.Println()
	return errors.Wrap(err, "load")
}
