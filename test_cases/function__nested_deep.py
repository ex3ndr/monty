def level1():
    def level2():
        def level3():
            return 42

        return level3()

    return level2()


level1()
# Return=42
