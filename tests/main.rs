use monty::{Executor, Exit};

macro_rules! parse_error_tests {
    ($($name:ident: $code:literal, $expected:literal;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< parse_error_ $name >]() {
                    match Executor::new($code, "test.py", &[]) {
                        Ok(v) => panic!("parse unexpected passed, output: {v:?}"),
                        Err(e) => assert_eq!(e.summary(), $expected),
                    }
                }
            }
        )*
    }
}

parse_error_tests! {
    complex: "1+2j", "TODO: complex constants";
}

macro_rules! execute_ok_tests {
    ($($name:ident: $code:literal, $expected:expr;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< execute_ok_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]);
                    let output = match result {
                        Ok(Exit::Return(obj)) => format!("{}: {}", obj.type_str(), obj.repr()),
                        otherwise => panic!("Unexpected exit: {:?}", otherwise),
                    };
                    let expected = $expected.trim_matches('\n');
                    assert_eq!(output, expected);
                }
            }
        )*
    }
}

execute_ok_tests! {
    add_ints: "1 + 1", "int: 2";
    add_strs: "'a' + 'b'", "str: 'ab'";
    for_loop_str_append_assign_op: "
v = ''
for i in range(1000):
    if i % 13 == 0:
        v += 'x'
len(v)
", "int: 77";
    for_loop_str_append_assign: "
v = ''
for i in range(1000):
    if i % 13 == 0:
        v = v + 'x'
len(v)
", "int: 77";
    shared_list_append: "
a = [1]
b = a
b.append(2)
len(a)
", "int: 2";
    list_repr: "
a = [1, 2, 3]
repr(a)
", "str: '[1, 2, 3]'";
list_str: "
a = [1, 2, 3]
str(a)
", "str: '[1, 2, 3]'";
ints_equal: "(1 == 1, 1 == 2)", "tuple: (True, False)";
str_equal: "('aa' == 'aa', 'aa' == 'bb')", "tuple: (True, False)";
str_equal2: "('aa' == 'aa', 'aa' == 'aaa')", "tuple: (True, False)";
bytes_equal: "(b'aa' == b'aa', b'aa' == b'bb')", "tuple: (True, False)";
bytes_equal2: "(b'aa' == b'aa', b'aa' == b'aaa')", "tuple: (True, False)";
list_equal: "([1, 2] == [1, 2], [1] == [1, 2])", "tuple: (True, False)";
tuple_equal: "((1, 2) == (1, 2), (1, 2) == (2, 1))", "tuple: (True, False)";
discard_list: "
a = 1
[1, 2, 3]
a
", "int: 1";
}

macro_rules! execute_raise_tests {
    ($($name:ident: $code:literal, $expected_exc:expr;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< execute_raise_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]);
                    let output = match result {
                        Ok(Exit::Raise(exc_raise)) => {
                            format!("{}", exc_raise.exc)
                        }
                        otherwise => panic!("Unexpected raise: {:?}", otherwise),
                    };
                    let expected = $expected_exc.trim_matches('\n');
                    assert_eq!(output, expected);
                }
            }
        )*
    }
}

execute_raise_tests! {
    error_instance_str: "raise ValueError('testing')", "ValueError('testing')";
    raise_number: "raise 1 + 2", "TypeError('exceptions must derive from BaseException')";
    error_type: "raise TypeError", "TypeError()";
    error_no_args: "raise TypeError()", "TypeError()";
    error_string_arg: "raise TypeError('hello')", "TypeError('hello')";
    error_string_arg_quotes: "raise TypeError(\"hello 'there'\")", "TypeError(\"hello 'there'\")";
    // error_two_args: "raise ValueError('x', 1 + 2)", "ValueError('x', 3)";
}
