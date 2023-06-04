#![feature(test)]

extern crate test;

use test::{black_box, Bencher};

use monty::{Executor, Exit, Object};

use pyo3::prelude::*;

#[bench]
fn add_two_monty(bench: &mut Bencher) {
    let ex = Executor::new("1 + 2", "test.py", &[]).unwrap();
    let v = ex.run(vec![]).unwrap();
    match v {
        Exit::Return(Object::Int(v)) => assert_eq!(v, 3),
        _ => panic!("unexpected exit: {:?}", v),
    }

    bench.iter(|| {
        let r = match ex.run(vec![]).unwrap() {
            Exit::Return(Object::Int(v)) => v,
            _ => -1,
        };
        black_box(r);
    });
}

#[bench]
fn add_two_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let fun: PyObject = PyModule::from_code(
            py,
            "def main():
                return 1 + 2
            ",
            "test.py",
            "main",
        )
        .unwrap()
        .getattr("main")
        .unwrap()
        .into();

        let r = fun.call0(py).unwrap();
        let r: i64 = r.extract(py).unwrap();
        assert_eq!(r, 3);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

// language=Python
const LOOP_MOD_13_CODE: &str = "
v = ''
for i in range(1_000):
    if i % 13 == 0:
        v += 'x'
len(v)
";

#[bench]
fn loop_mod_13_monty(bench: &mut Bencher) {
    let ex = Executor::new(LOOP_MOD_13_CODE, "test.py", &[]).unwrap();
    let v = ex.run(vec![]).unwrap();
    match v {
        Exit::Return(Object::Int(v)) => assert_eq!(v, 77),
        _ => panic!("unexpected exit: {:?}", v),
    }

    bench.iter(|| {
        black_box(ex.run(vec![]).unwrap());
    });
}

#[bench]
fn loop_mod_13_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        let fun: PyObject = PyModule::from_code(
            py,
            // language=Python
            "def main():
                v = ''
                for i in range(1_000):
                    if i % 13 == 0:
                        v += 'x'
                return len(v)
            ",
            "test.py",
            "main",
        )
        .unwrap()
        .getattr("main")
        .unwrap()
        .into();

        let r = fun.call0(py).unwrap();
        let r: i64 = r.extract(py).unwrap();
        assert_eq!(r, 77);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

#[bench]
fn end_to_end_monty(bench: &mut Bencher) {
    bench.iter(|| {
        let ex = Executor::new(black_box("1 + 2"), "test.py", &[]).unwrap();
        black_box(ex.run(vec![]).unwrap());
    });
}

#[bench]
fn end_to_end_cpython(bench: &mut Bencher) {
    Python::with_gil(|py| {
        bench.iter(|| {
            let fun: PyObject = PyModule::from_code(py, black_box("def main():\n  return 1 + 2"), "test.py", "main")
                .unwrap()
                .getattr("main")
                .unwrap()
                .into();
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}
