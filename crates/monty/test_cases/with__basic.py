# call-external
from pathlib import Path

# Create test files
Path('/virtual/test_with.txt').write_text('hello world')
Path('/virtual/test_with2.txt').write_text('second file')

# === Basic with statement + open() ===
with open('/virtual/test_with.txt') as f:
    content = f.read()
    assert content == 'hello world', f'expected hello world, got {content!r}'

# f should be closed after with block
assert f.closed, 'file should be closed after with block'

# === with statement without as clause ===
with open('/virtual/test_with.txt'):
    x = 42
assert x == 42, 'body executed'

# === Nested with statements ===
with open('/virtual/test_with.txt') as f1:
    with open('/virtual/test_with2.txt') as f2:
        c1 = f1.read()
        c2 = f2.read()
        assert c1 == 'hello world', f'nested: expected hello world, got {c1!r}'
        assert c2 == 'second file', f'nested: expected second file, got {c2!r}'

assert f1.closed, 'f1 should be closed after nested with'
assert f2.closed, 'f2 should be closed after nested with'

# === File attributes ===
with open('/virtual/test_with.txt') as f:
    assert f.name == '/virtual/test_with.txt', f'name mismatch: {f.name!r}'
    assert f.mode == 'r', f'mode mismatch: {f.mode!r}'
    assert not f.closed, 'should not be closed inside with'

# === readline ===
Path('/virtual/lines.txt').write_text('line1\nline2\nline3')
with open('/virtual/lines.txt') as f:
    assert f.readline() == 'line1\n', 'first line'
    assert f.readline() == 'line2\n', 'second line'
    assert f.readline() == 'line3', 'third line (no trailing newline)'
    assert f.readline() == '', 'EOF returns empty string'
