import test from 'ava'

import { MontyComplete, MontyRepl, MontySnapshot } from '../wrapper'

test('feed preserves state without replay', (t) => {
  const repl = new MontyRepl()

  repl.feed('counter = 0')
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 1)
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 2)
})

test('constructor accepts scriptName option', (t) => {
  const repl = new MontyRepl({ scriptName: 'test.py' })
  t.is(repl.scriptName, 'test.py')
})

test('default scriptName is main.py', (t) => {
  const repl = new MontyRepl()
  t.is(repl.scriptName, 'main.py')
})

test('repl dump/load roundtrip', (t) => {
  const repl = new MontyRepl()
  repl.feed('x = 40')
  t.is(repl.feed('x = x + 1'), null)

  const serialized = repl.dump()
  const loaded = MontyRepl.load(serialized)

  t.is(loaded.feed('x + 1'), 42)
})

test('feedStart pauses and resumes external function calls', (t) => {
  const repl = new MontyRepl()

  let progress = repl.feedStart('value = get_value()\nvalue')
  t.true(progress instanceof MontySnapshot)
  t.is((progress as MontySnapshot).functionName, 'get_value')
  t.false((progress as MontySnapshot).isOsFunction)

  progress = (progress as MontySnapshot).resume({ returnValue: 41 })
  t.true(progress instanceof MontyComplete)
  t.is((progress as MontyComplete).output, 41)
  t.is(repl.feed('value + 1'), 42)
})

test('feedStart pauses and resumes OS calls', (t) => {
  const repl = new MontyRepl()

  let progress = repl.feedStart('from datetime import datetime\nstamp = datetime.now()\nstamp.year')
  t.true(progress instanceof MontySnapshot)
  t.is((progress as MontySnapshot).functionName, 'datetime.now')
  t.true((progress as MontySnapshot).isOsFunction)

  progress = (progress as MontySnapshot).resume({ returnValue: new Date(2032, 4, 6, 7, 8, 9, 10) })
  t.true(progress instanceof MontyComplete)
  t.is((progress as MontyComplete).output, 2032)
  t.is(repl.feed('stamp.month'), 5)
})
