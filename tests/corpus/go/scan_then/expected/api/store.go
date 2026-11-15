package api

import "context"

func Load(ctx context.Context, id string) error {
	return nil
}

func Save(ctx context.Context, id string, v int) error { return nil }

func Sum(a, b int) int { return a + b }

func helper(id string) error { return nil }
