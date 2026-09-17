package main

func leaf(x int) int {
	return x + 1
}

func middle(x int) int {
	return leaf(x) + leaf(x+1)
}

func root(x int) int {
	return middle(x)
}

// An anonymous closure: must not appear as a named callable.
func withClosure(v []int) []int {
	out := make([]int, 0, len(v))
	for _, n := range v {
		func() { out = append(out, n*2) }()
	}
	return out
}

// A method, to exercise SymbolKind::METHOD as well as FUNCTION.
type counter struct{ n int }

func (c *counter) bump() int {
	return leaf(c.n)
}

func main() {
	_ = root(1)
	_ = withClosure([]int{1, 2})
	_ = (&counter{}).bump()
}
