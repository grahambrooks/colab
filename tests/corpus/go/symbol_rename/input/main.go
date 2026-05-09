package main

type OldName struct {
	Field int
}

func (o *OldName) Method() int {
	return o.Field
}

func makeOldName() *OldName {
	return &OldName{Field: 42}
}

func main() {
	v := makeOldName()
	_ = v.Method()
}
