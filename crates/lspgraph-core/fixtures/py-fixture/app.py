def leaf(x):
    return x + 1


def middle(x):
    return leaf(x) + leaf(x + 1)


def root(x):
    return middle(x)


def with_lambda(values):
    return list(map(lambda n: n * 2, values))
