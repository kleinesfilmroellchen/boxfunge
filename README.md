# Boxfunge

> Befunge interpreter in Rust

Boxfunge (oxidized Befunge) is an experimental interpreter for the esoteric [Befunge](https://catseye.tc/view/Befunge-93/doc/Befunge-93.markdown) programming language. Befunge is one of the most well-known esoteric programming languages and intended to be hard to compile.

This implementation has a basic, but very fast interpreter as its core. Currently, a very complicated program, such as the self-interpreter included in this repo, can run at roughly 4ns per Befunge command (200 million steps per second) in release mode. In fact, the interpreter is so fast that an optimizing JIT compiler was removed as it ran about 20% slower than the interpreter on average. The entire interpreter executable (no shared library dependencies) is only a few hundred kilobytes large. Using the `-o` option, a Befunge program can be compiled into a standalone executable, which is even smaller in size (and probably a bit faster) than the interpreter.

Boxfunge supports standard Befunge-93, the original variant. However, it is planned to remove the 80x25 grid restriction via a command-line flag, which would allow the language to be Turing-complete. Support for Befunge-98 (a generalized extension with many advanced features like concurrency) may be added at some point.

To try out the interpreter, this repo contains a collection of programs that are also used for testing the interpreter's functionality. They are mostly taken from the Esolangs wiki.

## Installation and Usage

```sh
$ cargo build
$ cargo run -- arguments...
$ cargo test
```

### Command-line interface

```
Usage: boxfunge <input> [-p] [-s <language-standard>] [-i <stdin>]

Befunge-93 interpreter.

Positional Arguments:
  input             input file to read

Options:
  -p, --show-performance
                    collect and show performance metrics
  -s, --language-standard
                    language standard to use, for future compatibility. default:
                    98
  -i, --stdin       file to use as stdin for the program; particularly useful
                    with self-interpreters
  --help            display usage information
```
