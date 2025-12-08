# === String concatenation (+) ===
assert 'hello' + ' ' + 'world' == 'hello world', 'basic concat'
assert '' + 'test' == 'test', 'empty left concat'
assert 'test' + '' == 'test', 'empty right concat'
assert '' + '' == '', 'empty both concat'
assert 'a' + 'b' + 'c' + 'd' == 'abcd', 'multiple concat'

# === Augmented assignment (+=) ===
s = 'hello'
s += ' world'
assert s == 'hello world', 'basic iadd'

s = 'test'
s += ''
assert s == 'test', 'iadd empty'

s = 'a'
s += 'b'
s += 'c'
assert s == 'abc', 'multiple iadd'

s = 'ab'
s += s
assert s == 'abab', 'iadd self'

# === String length ===
assert len('') == 0, 'len empty'
assert len('a') == 1, 'len single'
assert len('hello') == 5, 'len basic'
assert len('hello world') == 11, 'len with space'
assert len('caf\xe9') == 4, 'len unicode'

# === String repr/str ===
assert repr('') == "''", 'empty string repr'
assert str('') == '', 'empty string str'

assert repr('hello') == "'hello'", 'string repr'
assert str('hello') == 'hello', 'string str'

assert repr('hello "world"') == '\'hello "world"\'', 'string with quotes repr'
assert str('hello "world"') == 'hello "world"', 'string with quotes str'
