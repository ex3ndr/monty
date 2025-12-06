def outer():
    def inner():
        return 10

    return inner() + inner() + inner()


outer()
# Return=30
