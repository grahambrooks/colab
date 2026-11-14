package worker

import (
	"log"
	"testing"
)

func TestWorker(t *testing.T) {
	t.Cleanup(func() {
		log.Println("cleaned up")
	})
	done := make(chan struct{})
	go func() {
		defer close(done)
		log.Println("in goroutine")
		if err := run(); err != nil {
			t.Error(err)
		}
	}()
	<-done
	t.Fatal("not in a goroutine")
}

func helper() {
	log.Println("not a test")
}
