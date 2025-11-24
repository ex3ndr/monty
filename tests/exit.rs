use monty::{Executor, Exit};

// Macro for testing successful TryFrom conversions
macro_rules! try_from_ok_tests {
    ($($name:ident: $code:literal => $ty:ty, $expected:expr;)*) => {
        $(
            paste::item! {
                #[test]
                #[allow(clippy::float_cmp)]
                fn [< try_from_ok_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]).unwrap();
                    match result {
                        Exit::Return(ref obj) => {
                            let value: $ty = obj.try_into().expect("conversion should succeed");
                            assert_eq!(value, $expected);
                        }
                        otherwise => panic!("Unexpected exit: {:?}", otherwise),
                    }
                }
            }
        )*
    }
}

// Macro for testing failed TryFrom conversions
macro_rules! try_from_err_tests {
    ($($name:ident: $code:literal => $ty:ty, $expected_err:expr;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< try_from_err_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]).unwrap();
                    match result {
                        Exit::Return(obj) => {
                            let err = TryInto::<$ty>::try_into(&obj).expect_err("conversion should fail");
                            assert_eq!(err.to_string(), $expected_err);
                        }
                        otherwise => panic!("Unexpected exit: {:?}", otherwise),
                    }
                }
            }
        )*
    }
}

// Macro for testing ReturnObject methods (str, repr, type_str)
macro_rules! return_object_method_tests {
    ($($name:ident: $code:literal, str=$str_expected:literal, repr=$repr_expected:literal, type_str=$type_expected:literal;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< method_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]).unwrap();
                    match result {
                        Exit::Return(obj) => {
                            assert_eq!(obj.str().as_ref(), $str_expected, "str() mismatch");
                            assert_eq!(obj.repr().as_ref(), $repr_expected, "repr() mismatch");
                            assert_eq!(obj.type_str(), $type_expected, "type_str() mismatch");
                        }
                        otherwise => panic!("Unexpected exit: {:?}", otherwise),
                    }
                }
            }
        )*
    }
}

// Macro for testing is_none and is_ellipsis
macro_rules! is_variant_tests {
    ($($name:ident: $code:literal, is_none=$is_none:expr, is_ellipsis=$is_ellipsis:expr;)*) => {
        $(
            paste::item! {
                #[test]
                fn [< is_variant_ $name >]() {
                    let mut ex = Executor::new($code, "test.py", &[]).unwrap();
                    let result = ex.run(vec![]).unwrap();
                    match result {
                        Exit::Return(obj) => {
                            assert_eq!(obj.is_none(), $is_none, "is_none() mismatch");
                            assert_eq!(obj.is_ellipsis(), $is_ellipsis, "is_ellipsis() mismatch");
                        }
                        otherwise => panic!("Unexpected exit: {:?}", otherwise),
                    }
                }
            }
        )*
    }
}

// Test successful TryFrom conversions
try_from_ok_tests! {
    int_to_i64: "42" => i64, 42;
    zero_to_i64: "0" => i64, 0;
    float_to_f64: "2.5" => f64, 2.5;
    int_to_f64: "42" => f64, 42.0;
    string_to_string: "'hello'" => String, "hello".to_string();
    empty_string_to_string: "''" => String, String::new();
    multiline_string_to_string: "'hello\\nworld'" => String, "hello\nworld".to_string();
    bool_true_to_bool: "True" => bool, true;
    bool_false_to_bool: "False" => bool, false;
}

// Test failed TryFrom conversions
try_from_err_tests! {
    string_to_i64: "'hello'" => i64, "expected int, got str";
    float_to_i64: "2.5" => i64, "expected int, got float";
    none_to_i64: "None" => i64, "expected int, got NoneType";
    list_to_i64: "[1, 2, 3]" => i64, "expected int, got list";
    int_to_string: "42" => String, "expected str, got int";
    none_to_string: "None" => String, "expected str, got NoneType";
    list_to_string: "[1, 2]" => String, "expected str, got list";
    int_to_bool: "1" => bool, "expected bool, got int";
    string_to_bool: "'true'" => bool, "expected bool, got str";
    none_to_bool: "None" => bool, "expected bool, got NoneType";
}

// Test ReturnObject methods
return_object_method_tests! {
    int: "42", str="42", repr="42", type_str="int";
    float: "2.5", str="2.5", repr="2.5", type_str="float";
    string: "'hello'", str="hello", repr="'hello'", type_str="str";
    string_with_quotes: r#"'hello "world"'"#, str=r#"hello "world""#, repr=r#"'hello "world"'"#, type_str="str";
    empty_string: "''", str="", repr="''", type_str="str";
    bool_true: "True", str="True", repr="True", type_str="bool";
    bool_false: "False", str="False", repr="False", type_str="bool";
    none: "None", str="None", repr="None", type_str="NoneType";
    ellipsis: "...", str="...", repr="...", type_str="ellipsis";
    list: "[1, 2, 3]", str="[1, 2, 3]", repr="[1, 2, 3]", type_str="list";
    empty_list: "[]", str="[]", repr="[]", type_str="list";
    tuple: "(1, 2)", str="(1, 2)", repr="(1, 2)", type_str="tuple";
    bytes: "b'hello'", str="b'[104, 101, 108, 108, 111]'", repr="b'[104, 101, 108, 108, 111]'", type_str="bytes";
}

// Test is_none and is_ellipsis
is_variant_tests! {
    none: "None", is_none=true, is_ellipsis=false;
    ellipsis: "...", is_none=false, is_ellipsis=true;
    int: "42", is_none=false, is_ellipsis=false;
    float: "3.14", is_none=false, is_ellipsis=false;
    string: "'hello'", is_none=false, is_ellipsis=false;
    bool_true: "True", is_none=false, is_ellipsis=false;
    bool_false: "False", is_none=false, is_ellipsis=false;
    list: "[1, 2]", is_none=false, is_ellipsis=false;
    tuple: "(1, 2)", is_none=false, is_ellipsis=false;
}
