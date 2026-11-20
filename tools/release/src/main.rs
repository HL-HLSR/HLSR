use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match hlsr_release::cli::run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hlsr-release: {error}");
            ExitCode::from(1)
        }
    }
}
