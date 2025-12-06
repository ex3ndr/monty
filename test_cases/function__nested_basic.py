def foo():
    def bar():
        return 1

    return bar() + 1


foo()
# Return=2
