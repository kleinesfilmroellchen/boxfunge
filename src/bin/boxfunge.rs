//! Normal Boxfunge executable.

use boxfunge::*;

fn main() {
    let args: Arguments = argh::from_env();

    if args.language_standard != LanguageStandard::Befunge93 {
        eprintln!("only Befunge-93 is currently supported");
        std::process::exit(1);
    }

    run_interpreter(args).unwrap();
}
