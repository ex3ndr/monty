# === Bytes length ===
assert len(b'') == 0, 'len empty'
assert len(b'hello') == 5, 'len basic'

# === Bytes repr/str ===
assert repr(b'hello') == "b'hello'", 'bytes repr'
assert str(b'hello') == "b'hello'", 'bytes str'

# === Various bytes repr cases ===
assert repr(b'') == "b''", 'empty bytes repr'
assert repr(b"it's") == 'b"it\'s"', 'single quote bytes repr'
assert repr(b'l1\nl2') == "b'l1\\nl2'", 'newline bytes repr'
assert repr(b'col1\tcol2') == "b'col1\\tcol2'", 'tab bytes repr'
assert repr(b'\x00\xff') == "b'\\x00\\xff'", 'non-printable bytes repr'
assert repr(b'back\\slash') == "b'back\\\\slash'", 'backslash bytes repr'
