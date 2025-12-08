# === Integer addition ===
assert 1 + 2 == 3, 'basic add'
assert 5 + 0 == 5, 'add zero'
assert 0 + 5 == 5, 'zero add'

# === Integer subtraction ===
assert 5 - 3 == 2, 'basic sub'
assert 5 - 0 == 5, 'sub zero'

# === Integer modulo ===
assert 10 % 3 == 1, 'basic mod'
assert 3 % 10 == 3, 'mod larger divisor'
assert 9 % 3 == 0, 'mod zero result'

# === Augmented assignment (+=) ===
x = 5
x += 3
assert x == 8, 'basic iadd'

# === Integer repr/str ===
assert repr(42) == '42', 'int repr'
assert str(42) == '42', 'int str'

# === Float repr/str ===
assert repr(2.5) == '2.5', 'float repr'
assert str(2.5) == '2.5', 'float str'
