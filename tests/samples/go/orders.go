// Package orders is the property suite's Go sample.
package orders

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"
)

// Status of an order.
type Status int

const (
	Pending Status = iota
	Paid
	Shipped
	Cancelled
)

func (s Status) String() string {
	switch s {
	case Pending:
		return "pending"
	case Paid:
		return "paid"
	case Shipped:
		return "shipped"
	default:
		return "cancelled"
	}
}

// Line is one product in an order.
type Line struct {
	SKU      string `json:"sku"`
	Quantity int    `json:"quantity"`
	Cents    int64  `json:"cents"`
}

// Order is a customer's order.
type Order struct {
	ID      string
	Lines   []Line
	Status  Status
	Placed  time.Time
	Comment string
}

// ErrNotFound is returned for an unknown order.
var ErrNotFound = errors.New("order not found")

// Total is the order's value in cents.
func (o *Order) Total() int64 {
	var total int64
	for _, line := range o.Lines {
		total += line.Cents * int64(line.Quantity)
	}
	return total
}

// Store keeps orders in memory.
type Store struct {
	mu     sync.Mutex
	orders map[string]*Order
	clock  func() time.Time
}

// NewStore makes an empty store.
func NewStore() *Store {
	return &Store{orders: make(map[string]*Order), clock: time.Now}
}

// Place records a new order.
func (s *Store) Place(ctx context.Context, id string, lines ...Line) (*Order, error) {
	if err := ctx.Err(); err != nil {
		return nil, fmt.Errorf("place %s: %w", id, err)
	}
	if len(lines) == 0 {
		return nil, errors.New("an order needs at least one line")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	order := &Order{ID: id, Lines: lines, Status: Pending, Placed: s.clock()}
	s.orders[id] = order
	return order, nil
}

// Get returns one order.
func (s *Store) Get(id string) (*Order, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	order, ok := s.orders[id]
	if !ok {
		return nil, fmt.Errorf("get %q: %w", id, ErrNotFound)
	}
	return order, nil
}

// Advance moves an order to its next status.
func (s *Store) Advance(id string) error {
	order, err := s.Get(id)
	if err != nil {
		return err
	}
	switch order.Status {
	case Pending:
		order.Status = Paid
	case Paid:
		order.Status = Shipped
	case Shipped, Cancelled:
		return fmt.Errorf("order %s is already %s", id, order.Status)
	}
	return nil
}

// Search finds orders whose comment holds every word.
func (s *Store) Search(words ...string) []*Order {
	s.mu.Lock()
	defer s.mu.Unlock()
	var found []*Order
	for _, order := range s.orders {
		comment := strings.ToLower(order.Comment)
		matched := true
		for _, word := range words {
			if !strings.Contains(comment, strings.ToLower(word)) {
				matched = false
				break
			}
		}
		if matched {
			found = append(found, order)
		}
	}
	sort.Slice(found, func(i, j int) bool { return found[i].ID < found[j].ID })
	return found
}

// Summary is one line per status.
func (s *Store) Summary() string {
	counts := map[Status]int{}
	s.mu.Lock()
	for _, order := range s.orders {
		counts[order.Status]++
	}
	s.mu.Unlock()
	var b strings.Builder
	for _, status := range []Status{Pending, Paid, Shipped, Cancelled} {
		fmt.Fprintf(&b, "%s: %d\n", status, counts[status])
	}
	return b.String()
}

// Notify calls each listener in its own goroutine and waits for them.
func Notify(order *Order, listeners []func(*Order) error) error {
	var wg sync.WaitGroup
	errs := make(chan error, len(listeners))
	for _, listen := range listeners {
		wg.Add(1)
		go func(listen func(*Order) error) {
			defer wg.Done()
			if err := listen(order); err != nil {
				errs <- err
			}
		}(listen)
	}
	wg.Wait()
	close(errs)
	var all []error
	for err := range errs {
		all = append(all, err)
	}
	return errors.Join(all...)
}

// Example places and advances an order.
func Example() {
	store := NewStore()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	order, err := store.Place(ctx, "A-1", Line{SKU: "MLK", Quantity: 2, Cents: 129})
	if err != nil {
		fmt.Println("place:", err)
		return
	}
	_ = store.Advance(order.ID)
	fmt.Println(order.Total(), store.Summary())
}
