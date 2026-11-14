package files

import (
	"log"
	"os"
	"testing"
)

func TestOpen(t *testing.T) {
	f, err := os.Open("testdata/a.txt")
	defer f.Close()
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		if _, err := f.Stat(); err != nil {
			t.Error(err)
		}
	}()
	go func(tt *testing.T) {
		tt.Error("never")
	}(t)
	<-done
	log.Println("enter cleanup")
}

func TestOrdered(t *testing.T) {
	g, err := os.Open("testdata/b.txt")
	if err != nil {
		t.Fatal(err)
	}
	defer g.Close()
}
