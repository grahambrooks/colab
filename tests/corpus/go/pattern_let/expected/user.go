package user

type User struct {
	name  string
	email string
}

func (u *User) Name() string { return u.name }

func (u *User) Email() string {
	return u.email
}

// Getaway is not a getter.
func (u *User) Getaway() string { return "beach" }

func Describe(u *User) string {
	return u.Name() + " <" + u.Email() + "> " + u.Getaway()
}
