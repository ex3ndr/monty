def outer(x):
    def inner(y):
        return y + y

    return inner(x) + 1


outer(5)
# Return=11
