def outer():
    def add():
        return 1

    def sub():
        return 2

    return add() + sub()


outer()
# Return=3
