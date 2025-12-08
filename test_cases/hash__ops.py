# === Hash consistency for hashable types ===
assert hash(42) == hash(42), 'int hash consistent'
assert hash('hello') == hash('hello'), 'str hash consistent'
assert hash(None) == hash(None), 'None hash consistent'
assert hash(True) == hash(True), 'True hash consistent'
assert hash(False) == hash(False), 'False hash consistent'
assert hash((1, 2, 3)) == hash((1, 2, 3)), 'tuple hash consistent'
