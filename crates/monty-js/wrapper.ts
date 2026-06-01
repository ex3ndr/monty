// Custom error classes that extend Error for proper JavaScript error handling.
// These wrap the native Rust classes to provide instanceof support.

import type {
  ExceptionInfo,
  ExceptionInput,
  FeedOptions as NativeFeedOptions,
  Frame,
  JsMontyObject,
  MountDirOptions,
  MontyOptions,
  NameLookupLoadOptions,
  NameLookupResumeOptions,
  ResourceLimits,
  ResumeOptions,
  RunOptions as NativeRunOptions,
  SnapshotLoadOptions,
  StartOptions as NativeStartOptions,
} from './index.js'

import {
  Monty as NativeMonty,
  MountDir,
  MontyRepl as NativeMontyRepl,
  MontySnapshot as NativeMontySnapshot,
  MontyOsCall as NativeMontyOsCall,
  MontyNameLookup as NativeMontyNameLookup,
  MontyComplete as NativeMontyComplete,
  MontyException as NativeMontyException,
  MontyTypingError as NativeMontyTypingError,
} from './index.js'

export type {
  MontyOptions,
  MountDirOptions,
  ResourceLimits,
  Frame,
  ExceptionInfo,
  ResumeOptions,
  ExceptionInput,
  SnapshotLoadOptions,
  NameLookupResumeOptions,
  NameLookupLoadOptions,
  JsMontyObject,
}

/** Options for running code. */
export interface RunOptions extends Omit<NativeRunOptions, 'mount'> {
  /** Filesystem mount(s) for the sandbox. */
  mount?: MountLike | MountLike[]
}

/** Options for starting execution. */
export interface StartOptions extends Omit<NativeStartOptions, 'mount'> {
  /** Filesystem mount(s) for the sandbox. */
  mount?: MountLike | MountLike[]
}

/** Options for REPL feed(). */
export interface FeedOptions extends Omit<NativeFeedOptions, 'mount'> {
  /** Callback invoked on each print() call. */
  printCallback?: (stream: string, text: string) => void
  /** Filesystem mount(s) for the sandbox. */
  mount?: MountLike | MountLike[]
}

export { MountDir }

type MaybePromise<T> = T | Promise<T>

export type MountLike = MountDir | VirtualMount

export type VirtualMountMode = 'read-only' | 'read-write'

export interface VirtualMountOptions {
  /** Access mode. Default: 'read-write'. */
  mode?: VirtualMountMode
  /** Optional cumulative write limit, counted in bytes. */
  writeBytesLimit?: number
}

export interface VirtualMountStat {
  type?: 'file' | 'directory' | 'symlink'
  mode?: number
  size?: number
  mtime?: number
  atime?: number
  ctime?: number
  ino?: number
  dev?: number
  nlink?: number
  uid?: number
  gid?: number
}

export interface VirtualMountBackend {
  exists?(path: string): MaybePromise<boolean>
  isFile?(path: string): MaybePromise<boolean>
  isDir?(path: string): MaybePromise<boolean>
  isSymlink?(path: string): MaybePromise<boolean>
  readText?(path: string): MaybePromise<string>
  readBytes?(path: string): MaybePromise<Uint8Array | Buffer>
  writeText?(path: string, data: string): MaybePromise<number | void>
  writeBytes?(path: string, data: Uint8Array | Buffer): MaybePromise<number | void>
  appendText?(path: string, data: string): MaybePromise<number | void>
  appendBytes?(path: string, data: Uint8Array | Buffer): MaybePromise<number | void>
  mkdir?(path: string, options: { parents: boolean; existOk: boolean }): MaybePromise<void>
  unlink?(path: string): MaybePromise<void>
  rmdir?(path: string): MaybePromise<void>
  rename?(src: string, dst: string): MaybePromise<void>
  iterdir?(path: string): MaybePromise<Array<string | { name?: string; path?: string }>>
  stat?(path: string): MaybePromise<VirtualMountStat | ReturnType<typeof statResult>>
  resolve?(path: string): MaybePromise<string>
  absolute?(path: string): MaybePromise<string>
  open?(path: string, mode: string): MaybePromise<ReturnType<typeof fileHandle> | void>
}

export class VirtualMountError extends Error {
  readonly typeName: string

  constructor(typeName: string, message: string) {
    super(message)
    this.name = typeName
    this.typeName = typeName
  }
}

export class VirtualMount {
  readonly virtualPath: string
  readonly backend: VirtualMountBackend
  readonly mode: VirtualMountMode
  readonly writeBytesLimit: number | null
  private _writeBytesUsed = 0

  constructor(virtualPath: string, backend: VirtualMountBackend, options: VirtualMountOptions = {}) {
    if (!backend || typeof backend !== 'object') {
      throw new TypeError('VirtualMount backend must be an object')
    }
    this.virtualPath = normalizeVirtualPath(virtualPath)
    this.backend = backend
    this.mode = options.mode ?? 'read-write'
    if (this.mode !== 'read-only' && this.mode !== 'read-write') {
      throw new TypeError("VirtualMount mode must be 'read-only' or 'read-write'")
    }
    if (options.writeBytesLimit != null && options.writeBytesLimit < 0) {
      throw new TypeError('writeBytesLimit must be non-negative')
    }
    this.writeBytesLimit = options.writeBytesLimit ?? null
  }

  get writeBytesUsed(): number {
    return this._writeBytesUsed
  }

  matches(path: string): boolean {
    return pathMatchesMount(normalizeVirtualPath(path), this.virtualPath)
  }

  assertWritable(path: string): void {
    if (this.mode === 'read-only') {
      throw new VirtualMountError('PermissionError', `Read-only file system: '${path}'`)
    }
  }

  chargeWrite(bytes: number): void {
    if (this.writeBytesLimit == null) {
      return
    }
    if (this._writeBytesUsed + bytes > this.writeBytesLimit) {
      throw new VirtualMountError('OSError', `write limit exceeded: ${this.writeBytesLimit} bytes`)
    }
    this._writeBytesUsed += bytes
  }

  repr(): string {
    return `VirtualMount('${this.virtualPath}', '${this.mode}')`
  }
}

export function montyPath(value: string): { __monty_type__: 'Path'; value: string } {
  return { __monty_type__: 'Path', value: normalizeVirtualPath(value) }
}

export function fileHandle(
  path: string,
  mode: string,
  position = 0,
): { __monty_type__: 'FileHandle'; path: string; mode: string; position: number } {
  return { __monty_type__: 'FileHandle', path: normalizeVirtualPath(path), mode, position }
}

export function statResult(stat: VirtualMountStat): {
  __monty_type__: 'StatResult'
  stMode: number
  stIno: number
  stDev: number
  stNlink: number
  stUid: number
  stGid: number
  stSize: number
  stAtime: number
  stMtime: number
  stCtime: number
} {
  const kind = stat.type ?? 'file'
  const defaultMode = kind === 'directory' ? 0o040755 : kind === 'symlink' ? 0o120777 : 0o100644
  const mtime = stat.mtime ?? 0
  return {
    __monty_type__: 'StatResult',
    stMode: stat.mode ?? defaultMode,
    stIno: stat.ino ?? 0,
    stDev: stat.dev ?? 0,
    stNlink: stat.nlink ?? (kind === 'directory' ? 2 : 1),
    stUid: stat.uid ?? 0,
    stGid: stat.gid ?? 0,
    stSize: stat.size ?? (kind === 'directory' ? 4096 : 0),
    stAtime: stat.atime ?? mtime,
    stMtime: mtime,
    stCtime: stat.ctime ?? mtime,
  }
}

/**
 * Alias for ResourceLimits (deprecated name).
 */
export type JsResourceLimits = ResourceLimits

/**
 * Base class for all Monty interpreter errors.
 *
 * This is the parent class for `MontySyntaxError`, `MontyRuntimeError`, and `MontyTypingError`.
 * Catching `MontyError` will catch any exception raised by Monty.
 */
export class MontyError extends Error {
  protected _typeName: string
  protected _message: string

  constructor(typeName: string, message: string) {
    super(message ? `${typeName}: ${message}` : typeName)
    this.name = 'MontyError'
    this._typeName = typeName
    this._message = message
    // Maintains proper stack trace for where our error was thrown (only available on V8)
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyError)
    }
  }

  /**
   * Returns information about the inner Python exception.
   */
  get exception(): ExceptionInfo {
    return {
      typeName: this._typeName,
      message: this._message,
    }
  }

  /**
   * Returns formatted exception string.
   * @param format - 'type-msg' for 'ExceptionType: message', 'msg' for just the message
   */
  display(format: 'type-msg' | 'msg' = 'msg'): string {
    switch (format) {
      case 'msg':
        return this._message
      case 'type-msg':
        return this._message ? `${this._typeName}: ${this._message}` : this._typeName
      default:
        throw new Error(`Invalid display format: '${format}'. Expected 'type-msg' or 'msg'`)
    }
  }
}

/**
 * Raised when Python code has syntax errors or cannot be parsed by Monty.
 *
 * The inner exception is always a `SyntaxError`. Use `display()` to get
 * formatted error output.
 */
export class MontySyntaxError extends MontyError {
  private _native: NativeMontyException | null

  constructor(messageOrNative: string | NativeMontyException) {
    if (typeof messageOrNative === 'string') {
      super('SyntaxError', messageOrNative)
      this._native = null
    } else {
      const exc = messageOrNative.exception
      super('SyntaxError', exc.message)
      this._native = messageOrNative
    }
    this.name = 'MontySyntaxError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontySyntaxError)
    }
  }

  /**
   * Returns formatted exception string.
   * @param format - 'type-msg' for 'SyntaxError: message', 'msg' for just the message
   */
  override display(format: 'type-msg' | 'msg' = 'msg'): string {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format)
    }
    return super.display(format)
  }
}

/**
 * Raised when Monty code fails during execution.
 *
 * Provides access to the traceback frames where the error occurred via `traceback()`,
 * and formatted output via `display()`.
 */
export class MontyRuntimeError extends MontyError {
  private _native: NativeMontyException | null
  private _tracebackString: string | null
  private _frames: Frame[] | null

  constructor(
    nativeOrTypeName: NativeMontyException | string,
    message?: string,
    tracebackString?: string,
    frames?: Frame[],
  ) {
    if (typeof nativeOrTypeName === 'string') {
      // Legacy constructor: (typeName, message, tracebackString, frames)
      super(nativeOrTypeName, message!)
      this._native = null
      this._tracebackString = tracebackString ?? null
      this._frames = frames ?? null
    } else {
      // New constructor: (nativeException)
      const exc = nativeOrTypeName.exception
      super(exc.typeName, exc.message)
      this._native = nativeOrTypeName
      this._tracebackString = null
      this._frames = null
    }
    this.name = 'MontyRuntimeError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyRuntimeError)
    }
  }

  /**
   * Returns the Monty traceback as an array of Frame objects.
   */
  traceback(): Frame[] {
    if (this._native) {
      return this._native.traceback()
    }
    return this._frames || []
  }

  /**
   * Returns formatted exception string.
   * @param format - 'traceback' for full traceback, 'type-msg' for 'ExceptionType: message', 'msg' for just the message
   */
  display(format: 'traceback' | 'type-msg' | 'msg' = 'traceback'): string {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format)
    }
    // Fallback for legacy constructor
    switch (format) {
      case 'traceback':
        return this._tracebackString || this.message
      case 'type-msg':
        return this._message ? `${this._typeName}: ${this._message}` : this._typeName
      case 'msg':
        return this._message
      default:
        throw new Error(`Invalid display format: '${format}'. Expected 'traceback', 'type-msg', or 'msg'`)
    }
  }
}

export type TypingDisplayFormat =
  | 'full'
  | 'concise'
  | 'azure'
  | 'json'
  | 'jsonlines'
  | 'rdjson'
  | 'pylint'
  | 'gitlab'
  | 'github'

/**
 * Raised when type checking finds errors in the code.
 *
 * This exception is raised when static type analysis detects type errors.
 * Use `displayDiagnostics()` to render rich diagnostics in various formats for tooling integration.
 * Use `display()` (inherited) for simple 'type-msg' or 'msg' formats.
 */
export class MontyTypingError extends MontyError {
  private _native: NativeMontyTypingError | null

  constructor(messageOrNative: string | NativeMontyTypingError, nativeError: NativeMontyTypingError | null = null) {
    if (typeof messageOrNative === 'string') {
      super('TypeError', messageOrNative)
      this._native = nativeError
    } else {
      const exc = messageOrNative.exception
      super('TypeError', exc.message)
      this._native = messageOrNative
    }
    this.name = 'MontyTypingError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyTypingError)
    }
  }

  /**
   * Renders rich type error diagnostics for tooling integration.
   *
   * @param format - Output format (default: 'full')
   * @param color - Include ANSI color codes (default: false)
   */
  displayDiagnostics(format: TypingDisplayFormat = 'full', color: boolean = false): string {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format, color)
    }
    return this._message
  }
}

/**
 * Wrapped Monty class that throws proper Error subclasses.
 */
export class Monty {
  private _native: NativeMonty

  /**
   * Creates a new Monty interpreter by parsing the given code.
   *
   * @param code - Python code to execute
   * @param options - Configuration options
   * @throws {MontySyntaxError} If the code has syntax errors
   * @throws {MontyTypingError} If type checking is enabled and finds errors
   */
  constructor(code: string, options?: MontyOptions) {
    const result = NativeMonty.create(code, options)

    if (result instanceof NativeMontyException) {
      // Check typeName to distinguish syntax errors from other exceptions
      if (result.exception.typeName === 'SyntaxError') {
        throw new MontySyntaxError(result)
      }
      throw new MontyRuntimeError(result)
    }
    if (result instanceof NativeMontyTypingError) {
      throw new MontyTypingError(result)
    }

    this._native = result
  }

  /**
   * Performs static type checking on the code.
   *
   * @param prefixCode - Optional code to prepend before type checking
   * @throws {MontyTypingError} If type checking finds errors
   */
  typeCheck(prefixCode?: string): void {
    const result = this._native.typeCheck(prefixCode)
    if (result instanceof NativeMontyTypingError) {
      throw new MontyTypingError(result)
    }
  }

  /**
   * Executes the code and returns the result.
   *
   * @param options - Execution options (inputs, limits)
   * @returns The result of the last expression
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  run(options?: RunOptions): JsMontyObject {
    const split = splitMounts(options?.mount)
    if (split.virtualMounts.length > 0) {
      return runMontySync(this, { ...options, splitMounts: split })
    }
    const nativeOptions = options ? { ...options, mount: split.nativeMount } : undefined
    const result = this._native.run(nativeOptions)
    if (result instanceof NativeMontyException) {
      throw new MontyRuntimeError(result)
    }
    return result
  }

  /**
   * Starts execution and returns a snapshot (paused at external call or name lookup) or completion.
   *
   * @param options - Execution options (inputs, limits)
   * @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
   *   name lookup, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  start(options?: StartOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    const split = splitMounts(options?.mount)
    const context = createDispatchContext(split)
    return advanceSync(this._startNative(options, split, context), context)
  }

  _startNative(
    options: StartOptions | undefined,
    split: SplitMounts,
    context: DispatchContext,
  ): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    const result = this._native.start({
      ...options,
      mount: split.nativeMount,
      pauseOsCalls: options?.pauseOsCalls || split.virtualMounts.length > 0,
    })
    return wrapStartResult(result, context)
  }

  /**
   * Serializes the Monty instance to a binary format.
   */
  dump(): Buffer {
    return this._native.dump()
  }

  /**
   * Deserializes a Monty instance from binary format.
   */
  static load(data: Buffer): Monty {
    const instance = Object.create(Monty.prototype) as Monty
    instance._native = NativeMonty.load(data)
    return instance
  }

  /** Returns the script name. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /** Returns the input variable names. */
  get inputs(): string[] {
    return this._native.inputs
  }

  /** Returns a string representation of the Monty instance. */
  repr(): string {
    return this._native.repr()
  }
}

/** Options for creating a new MontyRepl instance. */
export interface MontyReplOptions {
  /** Name used in tracebacks and error messages. Default: 'main.py' */
  scriptName?: string
  /** Resource limits applied to all snippet executions. */
  limits?: ResourceLimits
}

/**
 * Incremental no-replay REPL session.
 *
 * Create with `new MontyRepl()` then call `feed()` to execute snippets
 * incrementally against persistent state.
 */
export class MontyRepl {
  private _native: NativeMontyRepl

  /**
   * Creates an empty REPL session ready to receive snippets via `feed()`.
   *
   * @param options - Optional configuration (scriptName, limits)
   */
  constructor(options?: MontyReplOptions) {
    this._native = new NativeMontyRepl(options)
  }

  /** Returns the script name for this REPL session. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /**
   * Executes one incremental snippet.
   *
   * @param code - Snippet code to execute
   * @param options - Optional feed options (mount)
   * @returns Snippet output
   * @throws {MontyRuntimeError} If execution raises an exception
   */
  feed(code: string, options?: FeedOptions): JsMontyObject {
    const split = splitMounts(options?.mount)
    if (split.virtualMounts.length > 0) {
      let progress = this.feedStart(code, options)
      while (!(progress instanceof MontyComplete)) {
        if (progress instanceof MontyNameLookup) {
          progress = progress.resume()
          continue
        }
        if (progress instanceof MontyOsCall) {
          progress = advanceSync(progress, progress.context)
          continue
        }
        progress = progress.resume({
          exception: {
            type: 'RuntimeError',
            message: `External function '${progress.functionName}' called but REPL feed() does not support external functions`,
          },
        })
      }
      return progress.output
    }
    const nativeOptions = options ? { ...options, mount: split.nativeMount } : undefined
    const result = this._native.feed(code, nativeOptions)
    if (result instanceof NativeMontyException) {
      throw new MontyRuntimeError(result)
    }
    return result
  }

  /**
   * Starts one incremental snippet and returns a resumable progress object.
   *
   * @param code - Snippet code to execute
   * @param options - Optional feed options (mount, printCallback)
   * @returns MontySnapshot if paused at function call, MontyNameLookup if
   *   paused at name lookup, MontyOsCall if paused at OS call, MontyComplete if done
   * @throws {MontyRuntimeError} If execution raises an exception
   */
  feedStart(code: string, options?: FeedOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    const split = splitMounts(options?.mount)
    const context = createDispatchContext(split)
    const result = this._native.feedStart(code, {
      ...options,
      mount: split.nativeMount,
      pauseOsCalls: options?.pauseOsCalls || split.virtualMounts.length > 0,
    })
    return advanceSync(wrapStartResult(result, context), context)
  }

  /** Executes one incremental snippet with async virtual mount support. */
  async feedAsync(code: string, options?: FeedOptions): Promise<JsMontyObject> {
    const split = splitMounts(options?.mount)
    const context = createDispatchContext(split)
    let progress = wrapStartResult(
      this._native.feedStart(code, {
        ...options,
        mount: split.nativeMount,
        pauseOsCalls: options?.pauseOsCalls || split.virtualMounts.length > 0,
      }),
      context,
    )
    progress = await advanceAsync(progress, context)
    while (!(progress instanceof MontyComplete)) {
      if (progress instanceof MontyNameLookup) {
        progress = await progress.resumeAsync()
      } else if (progress instanceof MontyOsCall) {
        progress = await advanceAsync(progress, context)
      } else {
        progress = await progress.resumeAsync({
          exception: {
            type: 'RuntimeError',
            message: `External function '${progress.functionName}' called but REPL feedAsync() does not support external functions`,
          },
        })
      }
      progress = await advanceAsync(progress, context)
    }
    return progress.output
  }

  /** Serializes the REPL session to bytes. */
  dump(): Buffer {
    return this._native.dump()
  }

  /** Restores a REPL session from bytes. */
  static load(data: Buffer): MontyRepl {
    const native = NativeMontyRepl.load(data)
    const repl = Object.create(MontyRepl.prototype) as MontyRepl
    ;(repl as any)._native = native
    return repl
  }

  /** Returns a string representation of the REPL session. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Helper to wrap native start/resume results, throwing errors as needed.
 */
function wrapStartResult(
  result: NativeMontySnapshot | NativeMontyNameLookup | NativeMontyComplete | NativeMontyOsCall | NativeMontyException,
  context?: DispatchContext,
): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
  if (result instanceof NativeMontyException) {
    throw new MontyRuntimeError(result)
  }
  // Check MontyNameLookup before MontySnapshot — napi `Either4` may cause
  // false positives with `instanceof` if checked in the wrong order.
  if (result instanceof NativeMontyNameLookup) {
    return new MontyNameLookup(result, context)
  }
  if (result instanceof NativeMontySnapshot) {
    return new MontySnapshot(result, context)
  }
  if (result instanceof NativeMontyOsCall) {
    return new MontyOsCall(result, context)
  }
  if (result instanceof NativeMontyComplete) {
    return new MontyComplete(result)
  }
  throw new Error(`Unexpected result type from native binding: ${result}`)
}

/**
 * Represents paused execution waiting for an external function call return value.
 *
 * Contains information about the pending external function call and allows
 * resuming execution with the return value or an exception.
 */
export class MontySnapshot {
  private _native: NativeMontySnapshot
  private _context?: DispatchContext

  constructor(nativeSnapshot: NativeMontySnapshot, context?: DispatchContext) {
    this._native = nativeSnapshot
    this._context = context
  }

  /** Returns the name of the script being executed. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /** Returns the name of the external function being called. */
  get functionName(): string {
    return this._native.functionName
  }

  /** Returns the positional arguments passed to the external function. */
  get args(): JsMontyObject[] {
    return this._native.args
  }

  /** Returns the keyword arguments passed to the external function as an object. */
  get kwargs(): Record<string, JsMontyObject> {
    return this._native.kwargs as Record<string, JsMontyObject>
  }

  /**
   * Resumes execution with either a return value or an exception.
   *
   * @param options - Object with either `returnValue` or `exception`
   * @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
   *   name lookup, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  resume(options: ResumeOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    return advanceSync(this._resumeNative(options), this._context)
  }

  async resumeAsync(options: ResumeOptions): Promise<MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall> {
    return advanceAsync(this._resumeNative(options), this._context)
  }

  _resumeNative(options: ResumeOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    const result = this._native.resume(options)
    return wrapStartResult(result, this._context)
  }

  /**
   * Serializes the MontySnapshot to a binary format.
   */
  dump(): Buffer {
    return this._native.dump()
  }

  /**
   * Deserializes a MontySnapshot from binary format.
   */
  static load(data: Buffer, options?: SnapshotLoadOptions): MontySnapshot {
    const nativeSnapshot = NativeMontySnapshot.load(data, options)
    return new MontySnapshot(nativeSnapshot)
  }

  /** Returns a string representation of the MontySnapshot. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Represents paused execution waiting for a name to be resolved.
 *
 * The host should check if the variable name corresponds to a known value
 * (e.g., an external function). Call `resume()` with the value to continue
 * execution, or call `resume()` with no value to raise `NameError`.
 */
export class MontyNameLookup {
  private _native: NativeMontyNameLookup
  private _context?: DispatchContext

  constructor(nativeNameLookup: NativeMontyNameLookup, context?: DispatchContext) {
    this._native = nativeNameLookup
    this._context = context
  }

  /** Returns the name of the script being executed. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /** Returns the name of the variable being looked up. */
  get variableName(): string {
    return this._native.variableName
  }

  /**
   * Resumes execution after resolving the name lookup.
   *
   * If `value` is provided, the name resolves to that value and execution continues.
   * If `value` is omitted/undefined, the VM raises a `NameError`.
   *
   * @param options - Optional object with `value` to resolve the name to
   * @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
   *   another name lookup, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  resume(options?: NameLookupResumeOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    return advanceSync(this._resumeNative(options), this._context)
  }

  async resumeAsync(
    options?: NameLookupResumeOptions,
  ): Promise<MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall> {
    return advanceAsync(this._resumeNative(options), this._context)
  }

  _resumeNative(options?: NameLookupResumeOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    const result = this._native.resume(options)
    return wrapStartResult(result, this._context)
  }

  /**
   * Serializes the MontyNameLookup to a binary format.
   */
  dump(): Buffer {
    return this._native.dump()
  }

  /**
   * Deserializes a MontyNameLookup from binary format.
   */
  static load(data: Buffer, options?: NameLookupLoadOptions): MontyNameLookup {
    const nativeLookup = NativeMontyNameLookup.load(data, options)
    return new MontyNameLookup(nativeLookup)
  }

  /** Returns a string representation of the MontyNameLookup. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Represents paused execution waiting for an OS or filesystem operation.
 */
export class MontyOsCall {
  private _native: NativeMontyOsCall
  private _context?: DispatchContext

  constructor(nativeOsCall: NativeMontyOsCall, context?: DispatchContext) {
    this._native = nativeOsCall
    this._context = context
  }

  get context(): DispatchContext | undefined {
    return this._context
  }

  /** Returns the name of the script being executed. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /** Returns the OS function name, such as `Path.read_text` or `Open`. */
  get functionName(): string {
    return this._native.functionName
  }

  /** Returns the positional arguments passed to the OS function. */
  get args(): JsMontyObject[] {
    return this._native.args
  }

  /** Returns the keyword arguments passed to the OS function as an object. */
  get kwargs(): Record<string, JsMontyObject> {
    return this._native.kwargs as Record<string, JsMontyObject>
  }

  /** Resumes execution with either an OS return value or an exception. */
  resume(options: ResumeOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    return advanceSync(this._resumeNative(options), this._context)
  }

  async resumeAsync(options: ResumeOptions): Promise<MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall> {
    return advanceAsync(this._resumeNative(options), this._context)
  }

  _resumeNative(options: ResumeOptions): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
    const result = this._native.resume(options)
    return wrapStartResult(result, this._context)
  }

  /** Serializes the OS-call snapshot to a binary format. */
  dump(): Buffer {
    return this._native.dump()
  }

  /** Deserializes a MontyOsCall from binary format. */
  static load(data: Buffer, options?: SnapshotLoadOptions): MontyOsCall {
    const nativeOsCall = NativeMontyOsCall.load(data, options)
    return new MontyOsCall(nativeOsCall)
  }

  /** Returns a string representation of the MontyOsCall. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Represents completed execution with a final output value.
 */
export class MontyComplete {
  private _native: NativeMontyComplete

  constructor(nativeComplete: NativeMontyComplete) {
    this._native = nativeComplete
  }

  /** Returns the final output value from the executed code. */
  get output(): JsMontyObject {
    return this._native.output
  }

  /** Returns a string representation of the MontyComplete. */
  repr(): string {
    return this._native.repr()
  }
}

interface SplitMounts {
  nativeMount?: MountDir | MountDir[]
  virtualMounts: VirtualMount[]
}

interface DispatchContext extends SplitMounts {
  externalFunctions?: Record<string, (...args: unknown[]) => unknown>
}

function createDispatchContext(
  split: SplitMounts,
  externalFunctions?: Record<string, (...args: unknown[]) => unknown>,
): DispatchContext {
  return { ...split, externalFunctions }
}

function splitMounts(mount?: MountLike | MountLike[] | SplitMounts): SplitMounts {
  if (isSplitMounts(mount)) {
    return mount
  }
  const native: MountDir[] = []
  const virtualMounts: VirtualMount[] = []
  const mounts = mount == null ? [] : Array.isArray(mount) ? mount : [mount]
  for (const item of mounts) {
    if (item instanceof MountDir) {
      native.push(item)
    } else if (item instanceof VirtualMount) {
      virtualMounts.push(item)
    } else {
      throw new TypeError('mount must be a MountDir, VirtualMount, or an array of mounts')
    }
  }
  virtualMounts.sort((a, b) => b.virtualPath.length - a.virtualPath.length)
  return {
    nativeMount: native.length === 0 ? undefined : native.length === 1 ? native[0] : native,
    virtualMounts,
  }
}

function isSplitMounts(value: unknown): value is SplitMounts {
  return (
    !!value &&
    typeof value === 'object' &&
    'virtualMounts' in value &&
    Array.isArray((value as SplitMounts).virtualMounts)
  )
}

function runMontySync(montyRunner: Monty, options: RunOptions & { splitMounts: SplitMounts }): JsMontyObject {
  const context = createDispatchContext(
    options.splitMounts,
    options.externalFunctions as Record<string, (...args: unknown[]) => unknown> | undefined,
  )
  let progress: MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall = montyRunner.start({
    inputs: options.inputs,
    limits: options.limits,
    printCallback: options.printCallback,
    mount: options.splitMounts as unknown as MountLike,
    pauseOsCalls: true,
  })

  while (!(progress instanceof MontyComplete)) {
    progress = advanceSync(progress, context)
    if (progress instanceof MontyComplete) {
      break
    }
    if (progress instanceof MontyNameLookup) {
      const extFunction = context.externalFunctions?.[progress.variableName]
      progress = extFunction ? progress.resume({ value: extFunction }) : progress.resume()
      continue
    }
    if (progress instanceof MontyOsCall) {
      continue
    }
    if (!(progress instanceof MontySnapshot)) {
      continue
    }
    const snapshot = progress
    const extFunction = context.externalFunctions?.[snapshot.functionName]
    if (!extFunction) {
      progress = snapshot.resume({
        exception: {
          type: 'NameError',
          message: `name '${snapshot.functionName}' is not defined`,
        },
      })
      continue
    }
    try {
      const result = extFunction(...snapshot.args, snapshot.kwargs)
      if (isPromiseLike(result)) {
        throw new TypeError('Monty.run() cannot await async external functions or virtual mount operations')
      }
      progress = snapshot.resume({ returnValue: result })
    } catch (error) {
      progress = snapshot.resume({ exception: exceptionFromError(error, 'RuntimeError') })
    }
  }

  return progress.output
}

function advanceSync(
  progress: MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall,
  context?: DispatchContext,
): MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall {
  if (!context || context.virtualMounts.length === 0) {
    return progress
  }
  let current = progress
  while (current instanceof MontyOsCall) {
    const osCall = current
    try {
      const result = dispatchVirtualMount(osCall, context)
      if (isPromiseLike(result)) {
        throw new TypeError('Use runMontyAsync() or MontyRepl.feedAsync() for async virtual mount operations')
      }
      current = osCall._resumeNative({ returnValue: result })
    } catch (error) {
      current = osCall._resumeNative({ exception: exceptionFromError(error, 'PermissionError') })
    }
  }
  return current
}

async function advanceAsync(
  progress: MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall,
  context?: DispatchContext,
): Promise<MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall> {
  if (!context || context.virtualMounts.length === 0) {
    return progress
  }
  let current = progress
  while (current instanceof MontyOsCall) {
    const osCall = current
    try {
      const result = await dispatchVirtualMount(osCall, context)
      current = osCall._resumeNative({ returnValue: result })
    } catch (error) {
      current = osCall._resumeNative({ exception: exceptionFromError(error, 'PermissionError') })
    }
  }
  return current
}

function dispatchVirtualMount(call: MontyOsCall, context: DispatchContext): MaybePromise<JsMontyObject> {
  const primaryPath = getPrimaryPath(call)
  const mount = context.virtualMounts.find((m) => m.matches(primaryPath))
  if (!mount) {
    throw new VirtualMountError('PermissionError', `Permission denied: '${primaryPath}'`)
  }
  const normalizedPath = normalizeVirtualPath(primaryPath)
  const backend = mount.backend

  switch (call.functionName) {
    case 'Path.exists':
      return callRequired(backend.exists, 'exists', normalizedPath)
    case 'Path.is_file':
      return callRequired(backend.isFile, 'isFile', normalizedPath)
    case 'Path.is_dir':
      return callRequired(backend.isDir, 'isDir', normalizedPath)
    case 'Path.is_symlink':
      return callRequired(backend.isSymlink, 'isSymlink', normalizedPath)
    case 'Path.read_text':
      return callRequired(backend.readText, 'readText', normalizedPath)
    case 'Path.read_bytes':
      return callRequired(backend.readBytes, 'readBytes', normalizedPath)
    case 'Path.write_text': {
      mount.assertWritable(normalizedPath)
      const data = getStringArg(call, 1, 'Path.write_text data')
      const bytes = Buffer.byteLength(data)
      mount.chargeWrite(bytes)
      return normalizeWriteResult(callRequired(backend.writeText, 'writeText', normalizedPath, data), [...data].length)
    }
    case 'Path.write_bytes': {
      mount.assertWritable(normalizedPath)
      const data = getBytesArg(call, 1, 'Path.write_bytes data')
      mount.chargeWrite(data.byteLength)
      return normalizeWriteResult(callRequired(backend.writeBytes, 'writeBytes', normalizedPath, data), data.byteLength)
    }
    case 'Path.append_text': {
      mount.assertWritable(normalizedPath)
      const data = getStringArg(call, 1, 'Path.append_text data')
      const bytes = Buffer.byteLength(data)
      mount.chargeWrite(bytes)
      return normalizeWriteResult(
        callRequired(backend.appendText, 'appendText', normalizedPath, data),
        [...data].length,
      )
    }
    case 'Path.append_bytes': {
      mount.assertWritable(normalizedPath)
      const data = getBytesArg(call, 1, 'Path.append_bytes data')
      mount.chargeWrite(data.byteLength)
      return normalizeWriteResult(
        callRequired(backend.appendBytes, 'appendBytes', normalizedPath, data),
        data.byteLength,
      )
    }
    case 'Path.mkdir':
      mount.assertWritable(normalizedPath)
      return normalizeNoneResult(
        callRequired(backend.mkdir, 'mkdir', normalizedPath, {
          parents: Boolean(call.kwargs.parents),
          existOk: Boolean(call.kwargs.exist_ok),
        }),
      )
    case 'Path.unlink':
      mount.assertWritable(normalizedPath)
      return normalizeNoneResult(callRequired(backend.unlink, 'unlink', normalizedPath))
    case 'Path.rmdir':
      mount.assertWritable(normalizedPath)
      return normalizeNoneResult(callRequired(backend.rmdir, 'rmdir', normalizedPath))
    case 'Path.rename': {
      mount.assertWritable(normalizedPath)
      const dst = normalizeVirtualPath(String(call.args[1]))
      if (!mount.matches(dst)) {
        throw new VirtualMountError('OSError', `Invalid cross-mount rename from '${normalizedPath}' to '${dst}'`)
      }
      return normalizeNoneResult(callRequired(backend.rename, 'rename', normalizedPath, dst))
    }
    case 'Path.iterdir':
      return normalizeIterdir(callRequired(backend.iterdir, 'iterdir', normalizedPath), normalizedPath)
    case 'Path.stat':
      return normalizeStatResult(callRequired(backend.stat, 'stat', normalizedPath))
    case 'Path.resolve':
      return normalizePathResult(callOptional(backend.resolve, normalizedPath), normalizedPath)
    case 'Path.absolute':
      return normalizePathResult(callOptional(backend.absolute, normalizedPath), normalizedPath)
    case 'Open':
      return handleVirtualOpen(mount, normalizedPath, getStringArg(call, 1, 'open mode'))
    default:
      throw new VirtualMountError('RuntimeError', `'${call.functionName}' is not supported by VirtualMount`)
  }
}

function callRequired<TArgs extends unknown[], TResult>(
  fn: ((...args: TArgs) => MaybePromise<TResult>) | undefined,
  name: string,
  ...args: TArgs
): MaybePromise<TResult> {
  if (!fn) {
    throw new VirtualMountError('NotImplementedError', `VirtualMount backend does not implement ${name}()`)
  }
  return fn(...args)
}

function callOptional<T>(
  fn: ((path: string) => MaybePromise<T>) | undefined,
  path: string,
): MaybePromise<T | undefined> {
  return fn?.(path)
}

function normalizeWriteResult(result: MaybePromise<number | void>, fallback: number): MaybePromise<number> {
  if (isPromiseLike(result)) {
    return result.then((value) => value ?? fallback)
  }
  return result ?? fallback
}

function normalizeNoneResult(result: MaybePromise<void>): MaybePromise<null> {
  return isPromiseLike(result) ? result.then(() => null) : null
}

function normalizeIterdir(
  result: MaybePromise<Array<string | { name?: string; path?: string }>>,
  parentPath: string,
): MaybePromise<Array<ReturnType<typeof montyPath>>> {
  const convert = (entries: Array<string | { name?: string; path?: string }>) =>
    entries.map((entry) => {
      const raw = typeof entry === 'string' ? entry : (entry.path ?? entry.name)
      if (!raw) {
        throw new VirtualMountError('RuntimeError', 'VirtualMount iterdir() entries must include a name or path')
      }
      return montyPath(raw.startsWith('/') ? raw : joinVirtualPath(parentPath, raw))
    })
  return isPromiseLike(result) ? result.then(convert) : convert(result)
}

function normalizeStatResult(
  result: MaybePromise<VirtualMountStat | ReturnType<typeof statResult>>,
): MaybePromise<ReturnType<typeof statResult>> {
  const convert = (value: VirtualMountStat | ReturnType<typeof statResult>) =>
    '__monty_type__' in value ? value : statResult(value)
  return isPromiseLike(result) ? result.then(convert) : convert(result)
}

function normalizePathResult(
  result: MaybePromise<string | undefined>,
  fallback: string,
): MaybePromise<ReturnType<typeof montyPath>> {
  const convert = (value: string | undefined) => montyPath(value ?? fallback)
  return isPromiseLike(result) ? result.then(convert) : convert(result)
}

function handleVirtualOpen(
  mount: VirtualMount,
  path: string,
  mode: string,
): MaybePromise<ReturnType<typeof fileHandle>> {
  if (mode.startsWith('w') || mode.startsWith('a')) {
    mount.assertWritable(path)
  }
  const custom = mount.backend.open?.(path, mode)
  if (custom !== undefined) {
    const normalize = (value: ReturnType<typeof fileHandle> | void) => value ?? fileHandle(path, mode)
    return isPromiseLike(custom) ? custom.then(normalize) : normalize(custom)
  }
  const binary = mode.includes('b')
  if (mode.startsWith('r')) {
    const exists = mount.backend.exists?.(path)
    const validate = (value: boolean | undefined) => {
      if (value === false) {
        throw new VirtualMountError('FileNotFoundError', `No such file or directory: '${path}'`)
      }
      return fileHandle(path, mode)
    }
    return isPromiseLike(exists) ? exists.then(validate) : validate(exists)
  }
  if (mode.startsWith('w')) {
    mount.chargeWrite(0)
    const result = binary
      ? callRequired(mount.backend.writeBytes, 'writeBytes', path, Buffer.alloc(0))
      : callRequired(mount.backend.writeText, 'writeText', path, '')
    return isPromiseLike(result) ? result.then(() => fileHandle(path, mode)) : fileHandle(path, mode)
  }
  if (mode.startsWith('a')) {
    const result = binary
      ? callRequired(mount.backend.appendBytes, 'appendBytes', path, Buffer.alloc(0))
      : callRequired(mount.backend.appendText, 'appendText', path, '')
    return isPromiseLike(result) ? result.then(() => fileHandle(path, mode)) : fileHandle(path, mode)
  }
  throw new VirtualMountError('ValueError', `invalid mode: '${mode}'`)
}

function getPrimaryPath(call: MontyOsCall): string {
  const path = call.args[0]
  if (typeof path !== 'string') {
    throw new VirtualMountError('RuntimeError', `${call.functionName} did not provide a path argument`)
  }
  return path
}

function getStringArg(call: MontyOsCall, index: number, label: string): string {
  const value = call.args[index]
  if (typeof value !== 'string') {
    throw new VirtualMountError('TypeError', `${label} must be a string`)
  }
  return value
}

function getBytesArg(call: MontyOsCall, index: number, label: string): Buffer {
  const value = call.args[index]
  if (Buffer.isBuffer(value)) {
    return value
  }
  if (value instanceof Uint8Array) {
    return Buffer.from(value)
  }
  throw new VirtualMountError('TypeError', `${label} must be bytes`)
}

function exceptionFromError(error: unknown, fallbackType: string): ExceptionInput {
  const err = error as Error & { typeName?: string }
  return {
    type: err.typeName || err.name || fallbackType,
    message: err.message || String(error),
  }
}

function isPromiseLike(value: unknown): value is Promise<unknown> {
  return (
    !!value &&
    (typeof value === 'object' || typeof value === 'function') &&
    typeof (value as Promise<unknown>).then === 'function'
  )
}

function normalizeVirtualPath(path: string): string {
  if (!path.startsWith('/')) {
    throw new TypeError(`virtual path must be absolute: '${path}'`)
  }
  const out: string[] = []
  for (const part of path.split('/')) {
    if (!part || part === '.') {
      continue
    }
    if (part === '..') {
      out.pop()
    } else {
      out.push(part)
    }
  }
  return `/${out.join('/')}`
}

function pathMatchesMount(path: string, mountPath: string): boolean {
  return path === mountPath || (mountPath === '/' ? path.startsWith('/') : path.startsWith(`${mountPath}/`))
}

function joinVirtualPath(parent: string, child: string): string {
  return normalizeVirtualPath(`${parent.replace(/\/+$/, '')}/${child.replace(/^\/+/, '')}`)
}

/**
 * Options for `runMontyAsync`.
 */
export interface RunMontyAsyncOptions {
  /** Input values for the script. */
  inputs?: Record<string, JsMontyObject>
  /** External function implementations (sync or async). */
  externalFunctions?: Record<string, (...args: unknown[]) => unknown>
  /** Resource limits. */
  limits?: ResourceLimits
  /** Callback invoked on each print() call. The first argument is the stream name (always "stdout"), the second is the printed text. */
  printCallback?: (stream: string, text: string) => void
  /** Filesystem mount(s) for the sandbox. */
  mount?: MountLike | MountLike[]
}

/**
 * Runs a Monty script with async external function support.
 *
 * This function handles both synchronous and asynchronous external functions.
 * When an external function returns a Promise, it will be awaited before
 * resuming execution.
 *
 * @param montyRunner - The Monty runner instance to execute
 * @param options - Execution options
 * @returns The output of the Monty script
 * @throws {MontyRuntimeError} If the code raises an exception
 * @throws {MontySyntaxError} If the code has syntax errors
 *
 * @example
 * const m = new Monty('result = await fetch_data(url)', {
 *   inputs: ['url'],
 * });
 *
 * const result = await runMontyAsync(m, {
 *   inputs: { url: 'https://example.com' },
 *   externalFunctions: {
 *     fetch_data: async (url) => {
 *       const response = await fetch(url);
 *       return response.text();
 *     }
 *   }
 * });
 */
export async function runMontyAsync(montyRunner: Monty, options: RunMontyAsyncOptions = {}): Promise<JsMontyObject> {
  const { inputs, externalFunctions = {}, limits, printCallback, mount } = options
  const split = splitMounts(mount)
  const context = createDispatchContext(split, externalFunctions)

  let progress: MontySnapshot | MontyNameLookup | MontyComplete | MontyOsCall = montyRunner._startNative(
    {
      inputs,
      limits,
      printCallback,
      mount: split as unknown as MountLike,
      pauseOsCalls: split.virtualMounts.length > 0,
    },
    split,
    context,
  )
  progress = await advanceAsync(progress, context)

  while (!(progress instanceof MontyComplete)) {
    if (progress instanceof MontyOsCall) {
      progress = await advanceAsync(progress, context)
      continue
    }
    if (progress instanceof MontyNameLookup) {
      // Name lookup — check if the name is a known external function
      const name = progress.variableName
      const extFunction = externalFunctions[name]
      if (extFunction) {
        // Resolve the name as a function value
        progress = await progress.resumeAsync({ value: extFunction })
      } else {
        // Unknown name — resume with no value to raise NameError
        progress = await progress.resumeAsync()
      }
      progress = await advanceAsync(progress, context)
      continue
    }

    // MontySnapshot — external function call
    const snapshot = progress
    const funcName = snapshot.functionName
    const extFunction = externalFunctions[funcName]

    if (!extFunction) {
      // Function not found — this shouldn't normally happen since NameLookup
      // would have raised NameError, but handle it defensively
      progress = await snapshot.resumeAsync({
        exception: {
          type: 'NameError',
          message: `name '${funcName}' is not defined`,
        },
      })
      progress = await advanceAsync(progress, context)
      continue
    }

    try {
      // Call the external function
      let result = extFunction(...snapshot.args, snapshot.kwargs)

      // If the result is a Promise, await it
      if (result && typeof (result as Promise<unknown>).then === 'function') {
        result = await result
      }

      // Resume with the return value
      progress = await snapshot.resumeAsync({ returnValue: result })
    } catch (error) {
      // External function threw an exception - convert to Monty exception
      const err = error as Error
      const excType = err.name || 'RuntimeError'
      const excMessage = err.message || String(error)
      progress = await snapshot.resumeAsync({
        exception: {
          type: excType,
          message: excMessage,
        },
      })
    }
    progress = await advanceAsync(progress, context)
  }

  return progress.output
}
