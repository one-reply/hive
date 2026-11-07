//! One-shot SSH helper: stdout is reserved for the framed response.
fn main() -> std::process::ExitCode {
    match hive_core::remote::serve(&mut std::io::stdin().lock(), &mut std::io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
