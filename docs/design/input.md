# Reading files and standard input (draft 1)

What a program can read from outside: standard input and files. Built 2026-11-08 as a Build-phase feature (no
performance work; that waits for the Optimise phase). The OS layer it sits on is in [`runtime.md`](runtime.md).

## 1. What it looks like

```
import std.fs

fn run(path: str) -> void ! io_error {
    const text = fs.readText(path)?                // the whole file
    var f = fs.open(path)?                         // or in pieces
    loop {
        switch f.readLine!()? {
            case some(line): print(line)
            case none: break
        }
    }
}
```

| Module | Names |
|---|---|
| `std.io` | `readLine() -> string?`, `readAll() -> string`, `readBytes() -> [byte]` (standard input, a failed read ends the input); `tryReadLine()`, `tryReadAll()` (the same with the failure as an `io_error`); `eprint`, `eprintln`; the type `file` |
| `std.fs` | `readText(path)`, `readBytes(path)`, `writeText(path, text)`, `appendText(path, text)`, `open`, `create`, `append` (each `! io_error`) |
| `core` | `io_error { kind: io_kind, message }` and `io_kind` (`notFound`, `denied`, `invalid`, `other`) |
| `file` methods | `readLine!()`, `readAll!()`, `readBytes!()` (`! io_error`), `write!(text)` (`! io_error`) |

## 2. Decisions

1. **Errors are values.** Everything that can fail on the outside world gives a `result` with an `io_error`; nothing
   stops the program. `kind` is for the cases a program tells apart (a missing file, no permission, a bad path);
   `message` is for people and names the path. `fs.readText("a\0b")` is `invalid`: no system accepts a zero byte in a
   path, so it is refused before the system call.
2. **A `file` is an owned value that closes itself.** `deinit` closes the descriptor, so there is no `close` to
   forget, and a file cannot be copied. Reading and writing are methods that change it (`f.readLine!()`).
3. **End of input and failure are different.** `readLine!()` is `string? ! io_error`: `none` is the end, `err` is
   a failed read. The short forms for standard input (`io.readLine`) treat a failure as the end, and the `try`
   forms give it back.
4. **Lines.** A line ends at `\n`; one `\r` before it is dropped; a last line without an ending still counts; an empty
   file has no lines.
5. **Text is not checked.** `readText` returns the bytes as a `string`; checking UTF-8 is a separate step (not
   built yet). `readBytes` gives `[byte]`.
6. **Writing is whole writes.** `writeText` creates or empties the file and writes once; `appendText` adds at the end.
   `file.write!` does not buffer: build the text first.
7. **Paths are UTF-8, passed through unchanged.** Windows converts them to UTF-16 for `CreateFileW`; forward slashes work
   on all targets. There is no path type yet (a later `std.path`).
8. **Standard input is one shared buffered `file`.** Short and long forms read from the same buffer, so mixing
   `readLine` and `readAll` loses nothing.

## 3. Memory, regions and binary size

- Every read returns a fresh owned `string` or `[byte]`; nothing borrows from the buffer, so no lifetimes reach the
  caller. A program that wants its input in an `arena` copies it in (arena values must not need dropping, and a
  `string` does).
- The read buffer (4 KiB chunks) is released whenever it is empty (`file.release`), so a file read to its end holds no
  memory and the leak check stays clean. A program that stops mid-buffer on standard input keeps one 4 KiB
  block until it ends.
- A program that never imports `std.io` or `std.fs` contains none of this: the module is linked only when imported
  and only the functions used are compiled in. A `fs.readText` program is about 7 KB; the short stdin forms do not
  build an `io_error` at all (`file.nextLine` and `file.rest` record the error number instead).

## 4. The OS layer

Two new operations beside `os_read` and `os_write` (`OsKind::Open`, `OsKind::Close`, IR names `os_open`, `os_close`):

- `os_open(path, mode)`: `path` is a zero-terminated block, `mode` is 0 (read), 1 (create or empty, write) or 2
  (append, create). Gives a descriptor of 3 or more, or `-4` (no such file), `-5` (permission denied) or `-3`.
- `os_close(fd)`: 0, or a negative error.

`os_read` and `os_write` take those descriptors as well as 0, 1 and 2. On Windows the descriptor is the `HANDLE`
itself (0, 1 and 2 map to the standard handles; a pipe whose writer is gone reads as the end of the input).

| Target | State |
|---|---|
| x86-64 Linux | `open`, `close`, `read`, `write` system calls |
| AArch64 Linux, macOS | `openat` (`open` on macOS), `close`, `read`, `write` |
| Windows (x86-64 and ARM64) | `CreateFileW` with a `MultiByteToWideChar` path, `CloseHandle`, `ReadFile`, `WriteFile` (five more `kernel32` imports) |
| Interpreter | the real file system through `std::fs`; standard input is read from the process a read at a time (`pelago run`) |

A directory opens for reading on Linux and fails at the first read (`is a directory`); on Windows it fails to open
(`permission denied`).

## 5. Not built (and why)

- `main(args)` outside x86-64 Linux (the start block is built there only); programs that use it are refused with a
  clear message on the other targets. Tests that need a directory pass it on standard input for that reason.
- Seek, file size, directories, `stat`, removing and renaming files, environment variables: the rest of the
  `os_*` set in `runtime.md` section 2.
- Typed JSON decoding from a file or from standard input (a library step over `readText`, tracked separately).
- Non-blocking and asynchronous reads (`concurrency-runtime.md`): a read blocks the whole program today, as tasks
  run one after another.
- Microcontrollers: a target without an operating system has no files; `std.fs` would not link there, and standard
  input is whatever the board's `os_read(0, ...)` is wired to (a UART). That binding belongs to the layout-file work
  for flat images.

## 6. Questions for the owner

1. Should `readText` check UTF-8 (a `utf8_error` kind) or keep returning unchecked bytes until a `text` type exists?
2. `io.readLine()` swallows read failures. Is that the right short form, or should every read give an `io_error`?
3. Do you want a buffered writer (`file.write!` is a system call each time) before the Optimise phase, or leave it?
