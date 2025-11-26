# Plan TODO Checklist

- [x] Phase 0: Land literal/l-value split and conversions.
- [x] Phase 0.5: Strip parse-time optimizations that require runtime objects; re-enable once heap semantics stabilize.
- [x] Phase 1: Introduce heap arena + reference counting fundamentals.
- [x] Phase 2: Thread heap through evaluator and execution engine.
- [x] Phase 3: Move heap-worthy types (list → str/bytes → dict/set/frozenset → tuple/exception) into `HeapData` and update cloning/assignment helpers.
- [x] Phase 3.5: Move heap-worthy Object variants (list/tuple/str/bytes/exception) into `HeapData`.
- [x] Phase 4: Support `is`/`is not` via object identity.
- [x] Phase 5: Update list/string mutation paths to work with shared references.
- [ ] Phase 6: Cycle detection (mark-sweep GC) — required before closures/classes per plan.md:859.
- [ ] Phase 7: String interning / other optimizations.
- [ ] Phase 8: Scope chain design (LEGB resolution) — needed for closures/global/nonlocal per plan.md:861.


Others:
* replace varied methods for checking argument counts
