//! Base file for embedded Befunge programs.
//! The program to be used gets passed as the `BEFUNGE_CODE_SRC` environment variable at compile time;
//! this is valid Rust source code for a Befunge 93 grid.

use std::io;

use boxfunge::*;

const CODE: Grid = include!(env!("BEFUNGE_CODE_SRC"));

fn main() {
    let input = Box::new(io::stdin());
    let output = Box::new(io::stdout());
    let mut interpreter = Interpreter::new_with_io_and_grid(CODE, input, output);

    interpreter.run_forever().unwrap();
}
