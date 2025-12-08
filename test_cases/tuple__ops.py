# === Tuple length ===
assert len(()) == 0, 'len empty'
assert len((1,)) == 1, 'len single'
assert len((1, 2, 3)) == 3, 'len basic'

# === Tuple indexing ===
a = (1, 2, 3)
assert a[1] == 2, 'getitem basic'

a = ('a', 'b', 'c')
assert a[0 - 2] == 'b', 'getitem negative'
assert a[-1] == 'c', 'getitem -1'

# === Nested tuples ===
assert ((1, 2), (3, 4)) == ((1, 2), (3, 4)), 'nested tuple'

# === Tuple repr/str ===
assert repr((1, 2)) == '(1, 2)', 'tuple repr'
assert str((1, 2)) == '(1, 2)', 'tuple str'
