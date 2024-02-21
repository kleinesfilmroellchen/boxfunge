//! Tests.

use std::path::Path;

use crate::run_interpreter;
use crate::Arguments;
use crate::Error;
use crate::Interpreter;

fn run_file(path: impl AsRef<Path>) -> Result<String, Error> {
    run_file_with_input(path, &[] as &[u8])
}

fn run_file_with_input(path: impl AsRef<Path>, input: &[u8]) -> Result<String, Error> {
    let source = std::fs::read_to_string(path.as_ref())?;
    let mut output = Vec::new();
    let input = Box::new(input);
    let output_box = Box::new(&mut output);
    let mut interpreter = Interpreter::new_with_io(&source, input, output_box)?;
    interpreter.run_forever()?;
    drop(interpreter);
    Ok(String::from_utf8_lossy(&output).to_string())
}

#[test]
fn hello_world() {
    assert_eq!(run_file("programs/hello_world.bf").unwrap(), "Hello World!");
}

#[test]
fn digiroot() {
    const DIGIROOT: &str = "programs/digiroot.bf";
    assert_eq!(run_file_with_input(DIGIROOT, b"9").unwrap().trim(), "9");
    assert_eq!(run_file_with_input(DIGIROOT, b"100").unwrap().trim(), "1");
    assert_eq!(run_file_with_input(DIGIROOT, b"88182").unwrap().trim(), "9");
    assert_eq!(run_file_with_input(DIGIROOT, b"91").unwrap().trim(), "1");
    assert_eq!(
        run_file_with_input(DIGIROOT, b"99999999").unwrap().trim(),
        "9"
    );
    assert_eq!(run_file_with_input(DIGIROOT, b"12").unwrap().trim(), "3");
    assert_eq!(run_file_with_input(DIGIROOT, b"123").unwrap().trim(), "6");
    assert_eq!(run_file_with_input(DIGIROOT, b"3004").unwrap().trim(), "7");
}

#[test]
fn randomness() {
    run_file_with_input("programs/rng.bf", b"0 19").unwrap();
    run_file("programs/dna1.bf").unwrap();
    run_file("programs/dna2.bf").unwrap();
}

#[test]
fn quines() {
    for quine_file in [
        "programs/kquine1.bf",
        "programs/kquine2.bf",
        "programs/kquine3.bf",
        "programs/kquine4.bf",
        "programs/kquine6.bf",
    ] {
        println!("checking quine {}", quine_file);
        assert_eq!(
            run_file(quine_file).unwrap().trim_end(),
            std::fs::read_to_string(quine_file).unwrap().trim_end()
        );
    }
}

#[test]
fn self_interpreter() {
    const SELF_INTERPRETER: &str = "programs/self_interpreter.bf";

    macro_rules! check_interpreters_equal {
        ($file:expr) => {
            assert_eq!(
                run_file_with_input(
                    SELF_INTERPRETER,
                    &std::fs::read_to_string($file).unwrap().into_bytes()
                )
                .unwrap(),
                run_file($file).unwrap()
            );
        };
    }

    // only check programs with deterministic output and without user input
    check_interpreters_equal!("programs/terminate.bf");
    check_interpreters_equal!("programs/hello_world.bf");
    check_interpreters_equal!("programs/kquine1.bf");
    check_interpreters_equal!("programs/kquine2.bf");
    check_interpreters_equal!("programs/primesieve.bf");
}

#[test]
fn cli() {
    run_interpreter(Arguments {
        input: "programs/hello_world.bf".into(),
        show_performance: false,
        language_standard: crate::LanguageStandard::Befunge93,
        stdin: None,
    })
    .unwrap();

    run_interpreter(Arguments {
        input: "programs/self_interpreter.bf".into(),
        show_performance: true,
        language_standard: crate::LanguageStandard::Befunge93,
        stdin: Some("programs/kquine3.bf".into()),
    })
    .unwrap();
}
