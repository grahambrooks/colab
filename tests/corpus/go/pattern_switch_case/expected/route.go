package route

func handler(version string) string {
	switch version {
	case "v1", "stable":
		return "v1"
	default:
		return "next"
	}
}
