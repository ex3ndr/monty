//! Compile-fail soundness tests for the `HeapReader` API.
//!
//! Each function here exercises a pattern that MUST be rejected by the borrow checker.
//! They are gated behind individual `cfg` flags so the integration test harness
//! (`tests/heap_reader_compile_fail.rs`) can compile each one independently and assert
//! that it fails with the expected error.
//!
//! These tests are never compiled during normal builds — only when the
//! `heap_reader_compile_fail_tests` cfg (plus a per-test cfg) is set.

use super::*;

/// Must not compile: allocating on the heap while holding a reference derived from `HeapRead::get`.
///
/// `a.get(&heap)` borrows `heap` immutably, and the resulting slice keeps that borrow alive.
/// `heap.heap_mut().allocate(...)` requires mutable access to `heap`, which
/// conflicts with the live immutable borrow.
///
/// Expected: E0502 (cannot borrow `*heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_heap_mutation_while_reading)]
fn heap_mutation_while_reading(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, &mut (), |mut heap, _| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let slice = a.get(&heap).as_slice();
        let _ = heap.heap_mut().allocate(HeapData::Str(Str::new("boom".into())));
        let _ = slice.len();
    });
}

/// Must not compile: two simultaneous `get_mut` calls produce aliasing `&mut` references.
///
/// `get_mut` requires `&mut HeapReader`, so a second `get_mut` while the first's return
/// value is still live creates a double mutable borrow.
///
/// Expected: E0499 (cannot borrow `heap` as mutable more than once at a time)
#[cfg(heap_reader_compile_fail_test_double_get_mut)]
fn double_get_mut(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, &mut (), |mut heap, _| {
        let mut a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let mut b = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let ref_a = a.get_mut(&mut heap);
        let ref_b = b.get_mut(&mut heap);
        let _ = (ref_a, ref_b);
    });
}

/// Must not compile: calling `dec_ref` while holding a `HeapRead`-derived reference.
///
/// `dec_ref` can free the entry (setting the slot to `None` and dropping the `HeapValue`),
/// which would leave the reference from `get()` dangling.
///
/// Expected: E0502 (cannot borrow `*heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_dec_ref_while_reading)]
fn dec_ref_while_reading(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, &mut (), |mut heap, _| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let list_ref = a.get(&heap);
        heap.heap_mut().dec_ref(list_id);
        let _ = list_ref.as_slice().len();
    });
}

/// Must not compile: smuggling a `HeapRead` out of the `HeapReader::with` closure.
///
/// The `for<'a>` bound on `HeapReader::with` means `'a` is universally quantified,
/// so `HeapRead<'a, _>` cannot outlive the closure.
///
/// Expected: E0521 (borrowed data escapes outside of closure)
#[cfg(heap_reader_compile_fail_test_smuggle_heap_read)]
fn smuggle_heap_read(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    let mut smuggled: Option<HeapRead<'_, List>> = None;
    HeapReader::with(heap, &mut (), |heap, _| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        smuggled = Some(a);
    });
    let _ = smuggled;
}

/// Must not compile: heap mutation inside a `.map()` closure while iterating over
/// data derived from `HeapRead::get`.
///
/// The iterator borrows the slice (which keeps `heap` immutably borrowed), and the
/// closure attempts to capture `heap` for mutable access.
///
/// Expected: E0502 (cannot borrow `heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_mutation_in_map_closure)]
fn mutation_in_map_closure(list_id: HeapId, other_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, &mut (), |mut heap, _| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let result: Vec<bool> = a
            .get(&heap)
            .as_slice()
            .iter()
            .map(|_v| {
                heap.heap_mut().dec_ref(other_id);
                true
            })
            .collect();
        let _ = result;
    });
}

/// Must not compile: calling `read()` (which takes `&mut self`) while holding a live
/// reference from `get()` (which borrows `self` as `&`).
///
/// Expected: E0502 (cannot borrow `heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_read_while_ref_alive)]
fn read_while_ref_alive(id_a: HeapId, id_b: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, &mut (), |mut heap, _| {
        let a = match heap.read(id_a) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let a_ref = a.get(&heap);
        let _b = heap.read(id_b);
        let _ = a_ref.as_slice();
    });
}

/// Must not compile: substituting one `HeapReader` for another via `HeapRead::get`.
///
/// Two nested `HeapReader::with` calls produce readers with different HRTB-bound
/// lifetimes (`'a` is invariant per call). A `HeapRead<'a, _>` from the outer
/// reader cannot be passed to the inner reader's `get`, even though both readers
/// are valid simultaneously — otherwise an attacker could feed a HeapRead from
/// one heap into another reader and dereference into the wrong arena.
///
/// Expected: E0521 (borrowed data escapes outside of closure) caused by the
/// outer `'a` being unable to satisfy the inner closure's universally-quantified
/// lifetime.
#[cfg(heap_reader_compile_fail_test_substitute_reader)]
fn substitute_reader(
    list_id: HeapId,
    heap_a: &mut Heap<impl ResourceTracker>,
    heap_b: &mut Heap<impl ResourceTracker>,
) {
    HeapReader::with(heap_a, &mut (), |reader_a, _| {
        let a = match reader_a.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        HeapReader::with(heap_b, &mut (), |reader_b, _| {
            // `a` came from `reader_a`; using it with `reader_b` would deref
            // into the wrong arena. Must be rejected.
            let _ = a.get(&reader_b);
        });
    });
}

/// Must not compile: substituting one `VM`'s heap for another `VM`'s heap.
///
/// VM construction goes through `HeapReader::with` + `VM::new`. The
/// brand-uniqueness guarantee from the with-call must still hold: a
/// `HeapRead<'h, _>` produced by one VM's heap reader cannot be used with a
/// different VM's reader (even when both VMs are alive).
///
/// Expected: E0521 (borrowed data escapes outside of closure) — the inner
/// closure's `'h` is universally quantified, so the outer VM's `'h` can't satisfy it.
#[cfg(heap_reader_compile_fail_test_substitute_vm)]
fn substitute_vm(
    list_id: HeapId,
    heap_a: &mut Heap<impl ResourceTracker>,
    heap_b: &mut Heap<impl ResourceTracker>,
    interns: &crate::intern::Interns,
) {
    use crate::bytecode::VM;
    HeapReader::with(
        heap_a,
        &mut (interns, crate::io::PrintWriter::Disabled),
        |reader_a, (interns, print)| {
            let vm_a = VM::new(reader_a, Vec::new(), *interns, None, print.reborrow());
            let a = match vm_a.heap.read(list_id) {
                HeapReadOutput::List(list) => list,
                _ => unreachable!(),
            };
            HeapReader::with(
                heap_b,
                &mut (*interns, crate::io::PrintWriter::Disabled),
                |reader_b, (interns, print)| {
                    let vm_b = VM::new(reader_b, Vec::new(), *interns, None, print.reborrow());
                    // `a` came from `vm_a`'s heap; using it with `vm_b` would deref
                    // into the wrong arena. Must be rejected.
                    let _ = a.get(&vm_b);
                },
            );
        },
    );
}

/// Must not compile: smuggling a `HeapRead` out of a `HeapReader::with` closure
/// via a captured `Option`.
///
/// The `for<'a>` bound on `with` makes `'a` universally quantified, so
/// neither the VM nor any `HeapRead<'a, _>` derived from it can outlive the
/// closure (the captured `Option<HeapRead<'_, _>>` would need `HeapRead<'static, _>`).
///
/// Expected: E0521 (borrowed data escapes outside of closure)
#[cfg(heap_reader_compile_fail_test_smuggle_heap_read_via_vm)]
fn smuggle_heap_read_via_vm(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>, interns: &crate::intern::Interns) {
    use crate::bytecode::VM;
    let mut smuggled: Option<HeapRead<'_, List>> = None;
    HeapReader::with(
        heap,
        &mut (interns, crate::io::PrintWriter::Disabled),
        |reader, (interns, print)| {
            let vm = VM::new(reader, Vec::new(), *interns, None, print.reborrow());
            let a = match vm.heap.read(list_id) {
                HeapReadOutput::List(list) => list,
                _ => unreachable!(),
            };
            smuggled = Some(a);
        },
    );
    let _ = smuggled;
}

/// Must not compile: returning the `VM` itself out of a `HeapReader::with` closure.
///
/// `VM<'h, T>` has invariant `'h`, and the closure's HRTB makes `'h`
/// universally quantified. Returning a value containing `'h` would require it
/// to satisfy any caller-chosen lifetime, which is impossible.
///
/// Expected: a borrow-check error preventing the VM from escaping.
#[cfg(heap_reader_compile_fail_test_smuggle_vm)]
fn smuggle_vm<T: ResourceTracker>(
    heap: &mut Heap<T>,
    interns: &crate::intern::Interns,
) -> crate::bytecode::VM<'static, T> {
    use crate::bytecode::VM;
    HeapReader::with(
        heap,
        &mut (interns, crate::io::PrintWriter::Disabled),
        |reader, (interns, print)| VM::new(reader, Vec::new(), *interns, None, print.reborrow()),
    )
}

/// Must not compile: cross-substituting between a bare `HeapReader::with`
/// reader and a VM constructed via `HeapReader::with`.
///
/// Both with-calls share the same HRTB brand mechanism; their lifetimes are
/// independent and incompatible by construction. A `HeapRead<'a, _>` from a
/// bare reader cannot be deref'd through a separately-constructed VM's heap
/// reader (and vice-versa).
///
/// Expected: E0521 (borrowed data escapes outside of closure)
#[cfg(heap_reader_compile_fail_test_substitute_reader_with_vm)]
fn substitute_reader_with_vm(
    list_id: HeapId,
    heap_a: &mut Heap<impl ResourceTracker>,
    heap_b: &mut Heap<impl ResourceTracker>,
    interns: &crate::intern::Interns,
) {
    use crate::bytecode::VM;
    HeapReader::with(heap_a, &mut (), |reader_a, _| {
        let a = match reader_a.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        HeapReader::with(
            heap_b,
            &mut (interns, crate::io::PrintWriter::Disabled),
            |reader_b, (interns, print)| {
                let vm_b = VM::new(reader_b, Vec::new(), *interns, None, print.reborrow());
                // `vm_b` impls `ContainsHeapReader` for its own (HRTB) lifetime,
                // not for `reader_a`'s lifetime. Cross-substitution must fail.
                let _ = a.get(&vm_b);
            },
        );
    });
}

/// Must not compile: smuggling an outer `HeapReader` into an inner
/// `HeapReader::with` call via the `data` channel and trying to `mem::swap`
/// the two readers.
///
/// The outer call brands its reader with one HRTB lifetime; the inner call
/// brands its reader with a fresh, independent HRTB lifetime. Both lifetimes
/// are invariant on `HeapReader`, so even though the structs are otherwise
/// identical, `HeapReader<'outer, T>` and `HeapReader<'inner, T>` are distinct
/// types and `mem::swap` cannot unify them.
///
/// If this attack worked, an attacker could swap the underlying `&mut Heap`
/// references between the two readers, letting `HeapRead<'outer, _>` from the
/// outer reader resolve into the inner reader's heap (or vice versa) — a
/// type-confusion across heap arenas.
///
/// Expected: lifetime/type mismatch error from `mem::swap` — the inner
/// closure's universally-quantified lifetime cannot be unified with the
/// outer call's lifetime.
#[cfg(heap_reader_compile_fail_test_smuggle_and_swap_reader)]
fn smuggle_and_swap_reader<T: ResourceTracker>(heap_a: &mut Heap<T>, heap_b: &mut Heap<T>) {
    HeapReader::with(heap_a, &mut (), |mut reader_a, _| {
        HeapReader::with(heap_b, &mut reader_a, |mut reader_b, smuggled| {
            // `reader_b: HeapReader<'inner, T>` and `*smuggled: HeapReader<'outer, T>`
            // — invariant lifetimes prevent unification, so this swap must be rejected.
            std::mem::swap(&mut reader_b, smuggled);
        });
    });
}
