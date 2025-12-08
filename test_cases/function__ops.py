# === Basic function calls ===
def f_no_args():
    return 1


assert f_no_args() == 1, 'no args'


def f_one_arg(x):
    return x


assert f_one_arg(42) == 42, 'one arg'


def add(a, b):
    return a + b


assert add(1, 2) == 3, 'two args'


def sum3(a, b, c):
    return a + b + c


assert sum3(1, 2, 3) == 6, 'three args'


# === Local variables ===
def f_local():
    x = 42
    return x


assert f_local() == 42, 'local var'


def f_local_from_arg(x):
    y = x + 1
    return y


assert f_local_from_arg(10) == 11, 'local var from arg'


def f_local_list():
    items = [1, 2, 3]
    return items


assert f_local_list() == [1, 2, 3], 'local var list'


def f_local_modify_list():
    items = [1, 2]
    items.append(3)
    return items


assert f_local_modify_list() == [1, 2, 3], 'local var modify list'


def f_local_multiple():
    a = 1
    b = 2
    c = 3
    return a + b + c


assert f_local_multiple() == 6, 'local var multiple'


def f_local_reassign():
    x = 1
    x = 2
    x = 3
    return x


assert f_local_reassign() == 3, 'local var reassign'


# === Nested functions ===
def nested_basic():
    def bar():
        return 1

    return bar() + 1


assert nested_basic() == 2, 'nested basic'


def nested_deep():
    def level2():
        def level3():
            return 42

        return level3()

    return level2()


assert nested_deep() == 42, 'nested deep'


def nested_multiple_calls():
    def inner():
        return 10

    return inner() + inner() + inner()


assert nested_multiple_calls() == 30, 'nested multiple calls'


def nested_two_inner():
    def add():
        return 1

    def sub():
        return 2

    return add() + sub()


assert nested_two_inner() == 3, 'nested two inner'


def nested_with_args(x):
    def inner(y):
        return y + y

    return inner(x) + 1


assert nested_with_args(5) == 11, 'nested with args'
