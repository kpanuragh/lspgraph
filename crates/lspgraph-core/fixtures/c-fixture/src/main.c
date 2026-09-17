int leaf(int x) {
	return x + 1;
}

int middle(int x) {
	return leaf(x) + leaf(x + 1);
}

int root(int x) {
	return middle(x);
}

/* A static function: internal linkage, a plausible awkward case. */
static int helper(int x) {
	return leaf(x);
}

int main(void) {
	return root(1) + helper(2);
}
