use criterion::{black_box, criterion_group, criterion_main, Bencher, Criterion};

use monty::Executor;

use pyo3::prelude::*;

/// Benchmarks adding two numbers using Monty interpreter
fn add_two_monty(bench: &mut Bencher) {
    let mut ex = Executor::new("1 + 2", "test.py", &[]).unwrap();
    let r = ex.run(vec![]).unwrap();

    let int_value: i64 = (&r.value().unwrap()).try_into().unwrap();
    assert_eq!(int_value, 3);

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let int_value: i64 = (&r.value().unwrap()).try_into().unwrap();
        black_box(int_value);
    });
}

/// Benchmarks adding two numbers using CPython
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

/// Benchmarks a loop with modulo operations using Monty interpreter
fn loop_mod_13_monty(bench: &mut Bencher) {
    let mut ex = Executor::new(LOOP_MOD_13_CODE, "test.py", &[]).unwrap();
    let r = ex.run(vec![]).unwrap();
    let int_value: i64 = (&r.value().unwrap()).try_into().unwrap();
    assert_eq!(int_value, 77);

    bench.iter(|| {
        let r = ex.run(vec![]).unwrap();
        let int_value: i64 = (&r.value().unwrap()).try_into().unwrap();
        black_box(int_value);
    });
}

/// Benchmarks a loop with modulo operations using CPython
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

/// Benchmarks end-to-end execution (parsing + running) using Monty
fn end_to_end_monty(bench: &mut Bencher) {
    bench.iter(|| {
        let mut ex = Executor::new(black_box("1 + 2"), "test.py", &[]).unwrap();
        black_box(ex.run(vec![]).unwrap());
    });
}

/// Benchmarks end-to-end execution (parsing + running) using CPython
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

/// Configures all benchmark groups
fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_two");
    group.bench_function("monty", add_two_monty);
    group.bench_function("cpython", add_two_cpython);
    group.finish();

    let mut group = c.benchmark_group("loop_mod_13");
    group.bench_function("monty", loop_mod_13_monty);
    group.bench_function("cpython", loop_mod_13_cpython);
    group.finish();

    let mut group = c.benchmark_group("end_to_end");
    group.bench_function("monty", end_to_end_monty);
    group.bench_function("cpython", end_to_end_cpython);
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
