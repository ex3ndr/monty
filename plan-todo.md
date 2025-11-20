# Plan TODO Checklist

- [x] Phase 0: Land literal/l-value split and conversions.
- [x] Phase 0.5: Strip parse-time optimizations that require runtime objects; re-enable once heap semantics stabilize.
- [x] Phase 1: Introduce heap arena + reference counting fundamentals.
- [x] Phase 2: Thread heap through evaluator and execution engine.
- [ ] Phase 3: Implement assignment/refcount semantics across namespaces.
- [ ] Phase 3.5: Move heap-worthy Object variants (list/tuple/str/bytes/exception) into `HeapData`.
- [ ] Phase 4: Support `is`/`is not` via object identity.
- [ ] Phase 5: Update list/string mutation paths to work with shared references.
- [ ] Phase 6: Move exceptions onto the heap.
- [ ] Phase 7: Optional integer caching (if still relevant with immediates).
- [ ] Phase 8: String interning / other optimizations.
