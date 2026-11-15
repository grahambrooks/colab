package main

func run(ctx context.Context) error {
	if err := api.Load("a"); err != nil {
		return err
	}
	_ = api.Sum(1, 2)
	_ = local.Load("x")
	return api.Save("a", 1)
}
