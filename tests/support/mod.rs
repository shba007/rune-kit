use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
pub fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rune-kit"))
}
// Shared by integration binaries; not every binary needs terminal fixtures.
#[allow(dead_code)]
pub fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

#[allow(dead_code)]
pub fn terminal_text(bytes: &[u8]) -> String {
    let mut parser = vt100::Parser::new(
        std::num::NonZeroU16::new(28).unwrap(),
        std::num::NonZeroU16::new(110).unwrap(),
        0,
    );
    let mut history = String::new();
    let mut previous = String::new();
    // Preserve rendered screens before a control sequence changes/clears them,
    // including the alternate screen before terminal restoration on exit.
    for chunk in bytes.split_inclusive(|byte| *byte == 0x1b) {
        parser.process(chunk);
        let screen = parser.screen().contents();
        if screen != previous {
            history.push_str(&screen);
            history.push('\n');
            previous = screen;
        }
    }
    history
}
pub fn output(command: &mut Command) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!("Test command exceeded 120 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}
