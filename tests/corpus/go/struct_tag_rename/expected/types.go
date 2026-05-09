package demo

type User struct {
	Name string `json:"new_name" yaml:"keep"`
	Age  int    `json:"age,omitempty"`
}

type Order struct {
	UserName string `json:"new_name"`
}

var unrelated = `json:"old_name"` // raw string literal, not a tag
