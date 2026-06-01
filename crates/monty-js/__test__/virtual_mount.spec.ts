import test from 'ava'

import { Monty, MontyRepl, MontyRuntimeError, VirtualMount, VirtualMountError, runMontyAsync } from '../wrapper'

function createMemoryMount(asyncBackend = false): VirtualMount {
  const files = new Map<string, Buffer>([
    ['/remote/hello.txt', Buffer.from('hello world')],
    ['/remote/subdir/nested.txt', Buffer.from('nested')],
  ])
  const dirs = new Set(['/remote', '/remote/subdir'])

  const delay = async <T>(value: T): Promise<T> => value
  const maybe = <T>(value: T): T | Promise<T> => (asyncBackend ? delay(value) : value)
  const requireFile = (path: string): Buffer => {
    const file = files.get(path)
    if (!file) {
      throw new VirtualMountError('FileNotFoundError', `No such file or directory: '${path}'`)
    }
    return file
  }

  return new VirtualMount('/remote', {
    exists: (path) => maybe(files.has(path) || dirs.has(path)),
    isFile: (path) => maybe(files.has(path)),
    isDir: (path) => maybe(dirs.has(path)),
    isSymlink: () => maybe(false),
    readText: (path) => maybe(requireFile(path).toString('utf8')),
    readBytes: (path) => maybe(Buffer.from(requireFile(path))),
    writeText: (path, data) => {
      files.set(path, Buffer.from(data))
      return maybe([...data].length)
    },
    writeBytes: (path, data) => {
      files.set(path, Buffer.from(data))
      return maybe(data.byteLength)
    },
    appendText: (path, data) => {
      const current = files.get(path) ?? Buffer.alloc(0)
      files.set(path, Buffer.concat([current, Buffer.from(data)]))
      return maybe([...data].length)
    },
    appendBytes: (path, data) => {
      const current = files.get(path) ?? Buffer.alloc(0)
      files.set(path, Buffer.concat([current, Buffer.from(data)]))
      return maybe(data.byteLength)
    },
    mkdir: (path) => {
      dirs.add(path)
      return maybe(undefined)
    },
    unlink: (path) => {
      files.delete(path)
      return maybe(undefined)
    },
    rmdir: (path) => {
      dirs.delete(path)
      return maybe(undefined)
    },
    rename: (src, dst) => {
      const file = requireFile(src)
      files.set(dst, file)
      files.delete(src)
      return maybe(undefined)
    },
    iterdir: (path) => {
      const prefix = `${path}/`
      const names = new Set<string>()
      for (const file of files.keys()) {
        if (file.startsWith(prefix)) {
          names.add(file.slice(prefix.length).split('/')[0])
        }
      }
      for (const dir of dirs) {
        if (dir.startsWith(prefix)) {
          names.add(dir.slice(prefix.length).split('/')[0])
        }
      }
      return maybe([...names].filter(Boolean).sort())
    },
    stat: (path) => {
      if (dirs.has(path)) {
        return maybe({ type: 'directory' as const, size: 4096, mtime: 1 })
      }
      const file = requireFile(path)
      return maybe({ type: 'file' as const, size: file.byteLength, mtime: 1 })
    },
  })
}

test('VirtualMount supports pathlib read write list stat and rename', (t) => {
  const mount = createMemoryMount()
  const code = `
from pathlib import Path
root = Path('/remote')
before = root.joinpath('hello.txt').read_text()
root.joinpath('new.txt').write_text('created')
root.joinpath('new.txt').rename('/remote/renamed.txt')
names = sorted([p.name for p in root.iterdir()])
size = root.joinpath('renamed.txt').stat().st_size
[before, names, size, root.joinpath('new.txt').exists(), root.joinpath('renamed.txt').read_text()]
`

  const result = new Monty(code).run({ mount })

  t.deepEqual(result, ['hello world', ['hello.txt', 'renamed.txt', 'subdir'], 7, false, 'created'])
})

test('VirtualMount supports open read and write', (t) => {
  const mount = createMemoryMount()
  const code = `
f = open('/remote/open.txt', 'w')
written = f.write('via open')
f.close()
g = open('/remote/open.txt', 'r')
content = g.read()
g.close()
[written, content]
`

  const result = new Monty(code).run({ mount })

  t.deepEqual(result, [8, 'via open'])
})

test('VirtualMount read-only mode blocks writes', (t) => {
  const backendMount = createMemoryMount()
  const mount = new VirtualMount('/remote', backendMount.backend, { mode: 'read-only' })

  const error = t.throws(
    () => new Monty("from pathlib import Path; Path('/remote/nope.txt').write_text('x')").run({ mount }),
    {
      instanceOf: MontyRuntimeError,
    },
  )

  t.true(error.message.includes('Read-only file system'))
})

test('runMontyAsync supports async VirtualMount backends', async (t) => {
  const mount = createMemoryMount(true)
  const code = "from pathlib import Path; Path('/remote/hello.txt').read_text()"

  const result = await runMontyAsync(new Monty(code), { mount })

  t.is(result, 'hello world')
})

test('MontyRepl feed supports VirtualMount persistence', (t) => {
  const mount = createMemoryMount()
  const repl = new MontyRepl()

  repl.feed('from pathlib import Path', { mount })
  repl.feed("Path('/remote/repl.txt').write_text('from repl')", { mount })
  const result = repl.feed("Path('/remote/repl.txt').read_text()", { mount })

  t.is(result, 'from repl')
})

test('MontyRepl feedAsync supports async VirtualMount backends', async (t) => {
  const mount = createMemoryMount(true)
  const repl = new MontyRepl()

  await repl.feedAsync('from pathlib import Path', { mount })
  await repl.feedAsync("Path('/remote/async.txt').write_text('async repl')", { mount })
  const result = await repl.feedAsync("Path('/remote/async.txt').read_text()", { mount })

  t.is(result, 'async repl')
})
